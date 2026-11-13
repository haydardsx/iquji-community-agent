use crate::{Agent, AgentError, Configuration, TurnEvent, TurnRequest, TurnResult};
use std::sync::atomic::AtomicBool;

pub(crate) fn run(
    agent: &Agent,
    configuration: &Configuration,
    request: TurnRequest,
    cancelled: &AtomicBool,
    on_event: impl Fn(TurnEvent) + Send + Sync + 'static,
) -> Result<TurnResult, AgentError> {
    #[cfg(not(unix))]
    {
        let _ = (agent, configuration, request, cancelled, on_event);
        Err(AgentError::PlatformUnsupported)
    }
    #[cfg(unix)]
    {
        unix::run(agent, configuration, request, cancelled, on_event)
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use crate::process_io::{Invocation, ProcessBudget};
    use serde_json::Value;
    use std::time::Instant;

    pub(super) fn run(
        agent: &Agent,
        configuration: &Configuration,
        request: TurnRequest,
        cancelled: &AtomicBool,
        on_event: impl Fn(TurnEvent),
    ) -> Result<TurnResult, AgentError> {
        if request
            .session_id
            .as_deref()
            .is_some_and(|id| !valid_thread(id))
        {
            return Err(AgentError::RequestInvalid);
        }
        let mut budget = ProcessBudget {
            deadline: Instant::now() + configuration.turn_timeout(),
            output_left: configuration.max_turn_output_bytes,
        };
        let program =
            crate::configuration_inspection::turn_executable(agent, cancelled, budget.deadline)?;
        let metadata_home = crate::cli_environment::CliEnvironment::isolated(agent.adapter)?;
        let (_, images_supported, model_supported) = crate::scan::codex_capabilities(|arguments| {
            crate::probe::probe_with_environment(
                &program,
                arguments,
                configuration,
                cancelled,
                &mut budget,
                &metadata_home.probe_values(),
            )
        })
        .map_err(turn_error)?;
        if request.model.is_some() && !model_supported {
            return Err(AgentError::ModelUnsupported);
        }
        if !request.attachments.is_empty() && !images_supported {
            return Err(AgentError::AttachmentUnsupported);
        }
        let images = crate::attachments::load(&request.attachments, cancelled, budget.deadline)?;
        let staged = if images.is_empty() {
            None
        } else {
            Some(crate::attachments::StagedImages::new(
                &images,
                cancelled,
                budget.deadline,
            )?)
        };
        let home = crate::cli_environment::CliEnvironment::new(agent)?;
        let mcp_home = if request.mcp_servers.is_empty() {
            None
        } else {
            Some(crate::codex_mcp::prepare_home(
                home.directory(),
                configuration,
                &request.mcp_servers,
                cancelled,
                budget.deadline,
            )?)
        };
        let refs = if (request.model.is_some() || request.context_fingerprint.is_some())
            && agent.codex_configuration_directory.is_some()
        {
            crate::model_codex::inspect(
                agent,
                cancelled,
                budget.deadline.min(
                    std::time::Instant::now()
                        + std::time::Duration::from_millis(configuration.timeout_ms),
                ),
            )
            .map_err(turn_error)?
            .credential_environment_variables
        } else {
            Vec::new()
        };
        let mut environment = home.turn_values_with_references(agent, &metadata_home, &refs)?;
        if let Some(mcp_home) = &mcp_home {
            environment.insert("CODEX_HOME".into(), mcp_home.path().as_os_str().to_owned());
        }
        let mut arguments = vec!["exec"];
        if let Some(model) = &request.model {
            arguments.extend(["-m", model]);
        }
        arguments.extend([
            "--json",
            "--color",
            "never",
            "--sandbox",
            if request.workspace_write {
                "workspace-write"
            } else {
                "read-only"
            },
            "--skip-git-repo-check",
            "-C",
        ]);
        let working_directory = configuration
            .working_directory
            .to_str()
            .ok_or(AgentError::ConfigurationInvalid)?;
        arguments.push(working_directory);
        if request.session_id.is_some() {
            arguments.push("resume");
        }
        if let Some(staged) = &staged {
            for path in staged.paths() {
                arguments.extend(["--image", path]);
            }
        }
        if let Some(session) = request.session_id.as_deref() {
            arguments.push(session);
        }
        arguments.push("-");
        let mut state = State {
            expected: request.session_id.as_deref(),
            session: None,
            started: false,
            completed: false,
            text: None,
            pending: Vec::new(),
        };
        if crate::configuration_inspection::turn_executable(agent, cancelled, budget.deadline)?
            != program
        {
            return Err(AgentError::ExecutableDigestMismatch);
        }
        crate::process_io::run(
            Invocation {
                program: &program,
                arguments: &arguments,
                input: Some(request.prompt.as_bytes()),
                environment: &environment,
                output_limit: configuration.max_turn_output_bytes,
            },
            configuration,
            cancelled,
            &mut budget,
            |bytes| state.chunk(bytes, &on_event),
        )
        .map_err(turn_error)?;
        if crate::configuration_inspection::turn_executable(agent, cancelled, budget.deadline)?
            != program
        {
            return Err(AgentError::ExecutableDigestMismatch);
        }
        if let Some(staged) = &staged {
            staged.verify(cancelled, budget.deadline)?;
        }
        if !state.pending.is_empty() {
            let last = std::mem::take(&mut state.pending);
            state.frame(&last, &on_event)?;
        }
        if !state.completed {
            return Err(AgentError::AgentRequestFailed);
        }
        let session_id = state.session.ok_or(AgentError::ProtocolInvalid)?;
        let text = state.text.ok_or(AgentError::AgentRequestFailed)?;
        // CLI 只保证完整 assistant item；不把工具事件、历史或进度伪造为 token 增量。
        on_event(TurnEvent::TextDelta { text: text.clone() });
        Ok(TurnResult {
            session_id: Some(session_id),
            resumable: true,
            text,
            stop_reason: "end_turn".into(),
        })
    }

    struct State<'a> {
        expected: Option<&'a str>,
        session: Option<String>,
        started: bool,
        completed: bool,
        text: Option<String>,
        pending: Vec<u8>,
    }

    impl State<'_> {
        fn chunk(&mut self, bytes: &[u8], emit: &impl Fn(TurnEvent)) -> Result<(), AgentError> {
            self.pending.extend_from_slice(bytes);
            while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<_> = self.pending.drain(..=end).collect();
                self.frame(&line, emit)?;
            }
            Ok(())
        }

        fn frame(&mut self, bytes: &[u8], emit: &impl Fn(TurnEvent)) -> Result<(), AgentError> {
            if bytes.iter().all(u8::is_ascii_whitespace) {
                return Ok(());
            }
            let value: Value =
                serde_json::from_slice(bytes).map_err(|_| AgentError::ProtocolInvalid)?;
            let kind = value
                .get("type")
                .and_then(Value::as_str)
                .ok_or(AgentError::ProtocolInvalid)?;
            match kind {
                "error" | "turn.failed" => {
                    let message = value
                        .get("message")
                        .or_else(|| value.get("error").and_then(|error| error.get("message")))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    return Err(
                        if [
                            "missing api key",
                            "no api key",
                            "missing credential",
                            "missing environment variable",
                        ]
                        .iter()
                        .any(|marker| message.contains(marker))
                        {
                            AgentError::ProviderCredentialMissing
                        } else {
                            AgentError::AgentRequestFailed
                        },
                    );
                }
                "thread.started" => {
                    let id = value
                        .get("thread_id")
                        .and_then(Value::as_str)
                        .filter(|id| valid_thread(id))
                        .ok_or(AgentError::ProtocolInvalid)?;
                    if self.expected.is_some_and(|expected| expected != id) {
                        return Err(AgentError::SessionMismatch);
                    }
                    if self.session.is_some() || self.started || self.completed {
                        return Err(AgentError::ProtocolInvalid);
                    }
                    self.session = Some(id.to_owned());
                    emit(TurnEvent::SessionStarted {
                        session_id: id.to_owned(),
                        resumable: true,
                    });
                }
                "turn.started" => {
                    if self.session.is_none() || self.started || self.completed {
                        return Err(AgentError::ProtocolInvalid);
                    }
                    self.started = true;
                }
                "item.completed" => {
                    let item = value.get("item").ok_or(AgentError::ProtocolInvalid)?;
                    if self.session.is_some()
                        && !self.started
                        && !self.completed
                        && item.get("type").and_then(Value::as_str) == Some("error")
                        && item.get("message").and_then(Value::as_str).is_some()
                    {
                        return Ok(());
                    }
                    if !self.started || self.completed {
                        return Err(AgentError::ProtocolInvalid);
                    }
                    if item.get("type").and_then(Value::as_str) == Some("agent_message") {
                        let text = item
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or(AgentError::ProtocolInvalid)?;
                        self.text = Some(
                            text.chars()
                                .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
                                .collect(),
                        );
                    }
                }
                "turn.completed" => {
                    if !self.started
                        || self.completed
                        || self.text.as_ref().is_none_or(|text| text.trim().is_empty())
                    {
                        return Err(AgentError::ProtocolInvalid);
                    }
                    self.completed = true;
                }
                _ => {}
            }
            Ok(())
        }
    }

    fn valid_thread(id: &str) -> bool {
        !id.is_empty()
            && id.len() <= 200
            && !id.starts_with('-')
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    }

    fn turn_error(error: AgentError) -> AgentError {
        match error {
            AgentError::ProbeTimedOut => AgentError::TurnTimedOut,
            AgentError::ProbeCancelled => AgentError::TurnCancelled,
            AgentError::ProbeOutputLimit => AgentError::TurnOutputLimit,
            error => error,
        }
    }
}

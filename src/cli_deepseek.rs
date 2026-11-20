use crate::{Agent, AgentError, Configuration, TurnRequest, TurnResult};
use std::sync::atomic::AtomicBool;

pub(crate) fn run(
    agent: &Agent,
    configuration: &Configuration,
    request: TurnRequest,
    cancelled: &AtomicBool,
) -> Result<TurnResult, AgentError> {
    // headless 每次都是独立回合，没有可恢复的线程；拒绝必须发生在任何子进程启动前。
    if request.session_id.is_some() {
        return Err(AgentError::SessionUnsupported);
    }
    #[cfg(not(unix))]
    {
        let _ = (agent, configuration, request, cancelled);
        Err(AgentError::PlatformUnsupported)
    }
    #[cfg(unix)]
    {
        use crate::cli_environment::CliEnvironment;
        use crate::process_io::{Invocation, ProcessBudget};
        let mut budget = ProcessBudget {
            deadline: std::time::Instant::now() + configuration.turn_timeout(),
            output_left: configuration.max_turn_output_bytes,
        };
        let program =
            crate::configuration_inspection::turn_executable(agent, cancelled, budget.deadline)?;
        let metadata_home = CliEnvironment::isolated(agent.adapter)?;
        let (version, configuration_supported) = crate::scan::deepseek_capabilities(|arguments| {
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
        if request.model.is_some() && !configuration_supported {
            return Err(AgentError::ModelUnsupported);
        }
        if !request.mcp_servers.is_empty() && !configuration_supported {
            return Err(AgentError::CapabilitiesUnsupported);
        }
        let home = CliEnvironment::new(agent)?;
        let restore_stored_authentication = version >= crate::version::Version(0, 2, 0)
            && agent.deepseek_configuration_directory.is_some();
        if restore_stored_authentication && !configuration_supported {
            return Err(AgentError::CapabilitiesUnsupported);
        }
        let inspect_configuration = restore_stored_authentication
            || !request.mcp_servers.is_empty()
            || request.model.is_some() && agent.deepseek_configuration_directory.is_some();
        let selected = if configuration_supported && inspect_configuration {
            Some(
                crate::model_deepseek::inspect(
                    agent,
                    configuration,
                    cancelled,
                    &mut budget,
                    &home.inspection_values(&metadata_home),
                )
                .map_err(turn_error)?,
            )
        } else {
            None
        };
        if request.model.is_some() && agent.deepseek_configuration_directory.is_none() {
            return Err(AgentError::ConfigurationMetadataUnavailable);
        }
        let refs = if agent.deepseek_configuration_directory.is_some() {
            selected
                .as_ref()
                .map(|value| value.metadata.credential_environment_variables.clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let model_override = if restore_stored_authentication
            || request.model.is_some()
            || !request.mcp_servers.is_empty()
        {
            Some(crate::model_deepseek::ModelOverride::create(
                selected.ok_or(AgentError::ConfigurationMetadataUnavailable)?,
                request.model.as_deref(),
                &request.mcp_servers,
                restore_stored_authentication,
                cancelled,
                budget.deadline,
            )?)
        } else {
            None
        };
        let mut environment = home.turn_values_with_references(agent, &metadata_home, &refs)?;
        environment.insert(
            "DSH_PERMISSION_MODE".into(),
            if request.workspace_write {
                "workspace-write".into()
            } else {
                "read-only".into()
            },
        );
        // headless 不读 stdin；前导空格保证任何提示词都只作为单一位置参数。
        let prompt = format!(" {}", request.prompt);
        let mut output = Vec::new();
        let mut errors = Vec::new();
        let mut arguments = vec!["--profile", "headless"];
        if let Some(selected) = &model_override {
            arguments.extend([
                "--patch",
                selected
                    .patch
                    .to_str()
                    .ok_or(AgentError::ConfigurationMetadataInvalid)?,
            ]);
        }
        arguments.push(&prompt);
        if let Some(selected) = &model_override {
            selected.verify(cancelled, budget.deadline)?;
        }
        if crate::configuration_inspection::turn_executable(agent, cancelled, budget.deadline)?
            != program
        {
            return Err(AgentError::ExecutableDigestMismatch);
        }
        let result = crate::process_io::run_observed(
            Invocation {
                program: &program,
                arguments: &arguments,
                input: None,
                environment: &environment,
                output_limit: configuration.max_turn_output_bytes,
            },
            configuration,
            cancelled,
            &mut budget,
            |bytes| {
                output.extend_from_slice(bytes);
                Ok(())
            },
            |bytes| {
                errors.extend_from_slice(bytes);
                Ok(())
            },
        );
        if let Err(error) = result {
            return Err(if error == AgentError::ProcessFailed {
                failed_request(&errors)
            } else {
                turn_error(error)
            });
        }
        if crate::configuration_inspection::turn_executable(agent, cancelled, budget.deadline)?
            != program
        {
            return Err(AgentError::ExecutableDigestMismatch);
        }
        if let Some(selected) = &model_override {
            selected.verify(cancelled, budget.deadline)?;
        }
        let text: String = String::from_utf8(output)
            .map_err(|_| AgentError::ProtocolInvalid)?
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect();
        if text.trim().is_empty() {
            return Err(AgentError::AgentRequestFailed);
        }
        // stdout 是整回合最终回复，不能伪造 session_started 或 token 增量。
        Ok(TurnResult {
            session_id: None,
            resumable: false,
            text,
            stop_reason: "end_turn".into(),
        })
    }
}

#[cfg(unix)]
fn failed_request(errors: &[u8]) -> AgentError {
    // 只投影首个正式 dsh 错误码，不透传 stderr 原文或 OS cause。
    let errors = String::from_utf8_lossy(errors);
    for line in errors.lines() {
        let Some(rest) = line.trim().strip_prefix("dsh: ") else {
            continue;
        };
        let Some((code, _)) = rest.split_once(':') else {
            continue;
        };
        return if code.trim() == "MISSING_CREDENTIAL" {
            AgentError::ProviderCredentialMissing
        } else {
            AgentError::AgentRequestFailed
        };
    }
    if invalid_credential_document(&errors) {
        return AgentError::ConfigurationMetadataInvalid;
    }
    AgentError::AgentRequestFailed
}

#[cfg(unix)]
fn invalid_credential_document(errors: &str) -> bool {
    // 只接受官方凭据解析器及加载器的同一启动堆栈，不泛化 TypeError 或读取凭据。
    let frame = |method: &str| {
        errors.lines().any(|line| {
            line.trim().strip_prefix("at ").is_some_and(|line| {
                line.starts_with(method)
                    && line.contains("@deepseek-ai/dsh-credentials-local/lib/index.js:")
            })
        })
    };
    frame("parseCredentialsDocument (")
        && frame("LocalCredentialProvider.loadInitial (")
        && errors.lines().any(|line| {
            let Some((_, detail)) =
                line.split_once("TypeError: credentials-local: the value for \"")
            else {
                return false;
            };
            let Some((name, detail)) = detail.split_once("\" in ") else {
                return false;
            };
            let Some(path) = detail.strip_suffix(" must be a string") else {
                return false;
            };
            !name.is_empty()
                && name.bytes().enumerate().all(|(index, byte)| {
                    byte.is_ascii_alphabetic() || byte == b'_' || index > 0 && byte.is_ascii_digit()
                })
                && std::path::Path::new(path).is_absolute()
                && path.ends_with("/.credentials.yaml")
        })
}

#[cfg(unix)]
fn turn_error(error: AgentError) -> AgentError {
    match error {
        AgentError::ProbeTimedOut => AgentError::TurnTimedOut,
        AgentError::ProbeCancelled => AgentError::TurnCancelled,
        AgentError::ProbeOutputLimit => AgentError::TurnOutputLimit,
        error => error,
    }
}

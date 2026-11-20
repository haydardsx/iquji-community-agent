use crate::{Adapter, AgentError, Configuration, TurnEvent, TurnRequest, TurnResult};
use iquji_agent_wire::valid_session_id;
use std::sync::atomic::AtomicBool;

pub fn run_turn(
    configuration: &Configuration,
    request: TurnRequest,
    cancelled: &AtomicBool,
    on_event: impl Fn(TurnEvent) + Send + Sync + 'static,
) -> Result<TurnResult, AgentError> {
    let deadline = std::time::Instant::now() + configuration.turn_timeout();
    request.validate()?;
    let agent = configuration
        .agents
        .iter()
        .find(|agent| agent.id == request.agent_id)
        .ok_or(AgentError::RequestInvalid)?;
    let pinned_digest = if let Some(expected) = &request.context_fingerprint {
        let (actual, digest) = crate::configuration_inspection::turn_context(
            agent,
            configuration,
            cancelled,
            deadline,
        )?;
        if *expected != actual {
            return Err(AgentError::ExecutableDigestMismatch);
        }
        Some(digest)
    } else {
        None
    };
    let mut remaining = configuration.clone();
    remaining.turn_timeout_ms = deadline
        .saturating_duration_since(std::time::Instant::now())
        .as_millis()
        .min(u64::MAX as u128) as u64;
    if remaining.turn_timeout_ms == 0 {
        return Err(AgentError::TurnTimedOut);
    }
    if let Some(digest) = pinned_digest {
        remaining
            .agents
            .iter_mut()
            .find(|agent| agent.id == request.agent_id)
            .ok_or(AgentError::RequestInvalid)?
            .expected_sha256 = Some(digest);
    }
    let configuration = &remaining;
    let agent = configuration
        .agents
        .iter()
        .find(|agent| agent.id == request.agent_id)
        .ok_or(AgentError::RequestInvalid)?;

    if agent.adapter == Adapter::DeepseekHarness && !request.attachments.is_empty() {
        return Err(AgentError::AttachmentUnsupported);
    }
    if agent.adapter == Adapter::CodexCli {
        return crate::cli_codex::run(agent, configuration, request, cancelled, on_event);
    }
    if agent.adapter == Adapter::DeepseekHarness {
        return crate::cli_deepseek::run(agent, configuration, request, cancelled);
    }
    if !matches!(
        agent.adapter,
        Adapter::NativeAcp | Adapter::ExternalAcpAdapter
    ) {
        return Err(AgentError::CapabilitiesUnsupported);
    }
    if request.model.is_some() {
        return Err(AgentError::ModelUnsupported);
    }
    let program = crate::configuration_inspection::turn_executable(agent, cancelled, deadline)?;
    #[cfg(unix)]
    {
        unix::run(&program, configuration, request, cancelled, on_event)
    }
    #[cfg(not(unix))]
    {
        let _ = (program, request, cancelled, on_event);
        Err(AgentError::PlatformUnsupported)
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use agent_client_protocol::schema::{
        ProtocolVersion,
        v1::{
            ContentBlock, ImageContent, InitializeRequest, LoadSessionRequest, McpServer,
            McpServerHttp, NewSessionRequest, PromptRequest, ReadTextFileRequest,
            RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
            SessionNotification, SessionUpdate, TextContent, WriteTextFileRequest,
        },
    };
    use agent_client_protocol::{Client, Error};
    use base64::Engine as _;
    use iquji_agent_wire::McpServer as WireMcpServer;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct State {
        session: Option<String>,
        prompting: bool,
        text: String,
        error: Option<AgentError>,
        result: Option<TurnResult>,
    }

    fn remember_failure(state: &Mutex<State>, error: Error) -> Error {
        let code = if error.code == agent_client_protocol::ErrorCode::AuthRequired {
            AgentError::AuthenticationRequired
        } else {
            AgentError::AgentRequestFailed
        };
        if let Ok(mut state) = state.lock() {
            state.error.get_or_insert(code);
        }
        // 只保留正式错误种类，不把对方 message/data 带入上层错误或日志。
        Error::internal_error()
    }

    pub(super) fn run(
        program: &Path,
        configuration: &Configuration,
        request: TurnRequest,
        cancelled: &AtomicBool,
        on_event: impl Fn(TurnEvent) + Send + Sync + 'static,
    ) -> Result<TurnResult, AgentError> {
        let deadline = std::time::Instant::now() + configuration.turn_timeout();
        let state = Arc::new(Mutex::new(State::default()));
        let updates = state.clone();
        let result = state.clone();
        let abort = state.clone();
        let on_event = Arc::new(on_event);
        let events = on_event.clone();
        if request.workspace_write {
            return Err(AgentError::CapabilitiesUnsupported);
        }
        let cwd = configuration.working_directory.clone();
        let output = crate::acp_transport::run(
            program,
            configuration,
            cancelled,
            std::time::Duration::from_millis(configuration.turn_timeout_ms),
            configuration.max_turn_output_bytes,
            move || abort.lock().ok().and_then(|state| state.error),
            move |streams| {
                Client
                    .builder()
                    .name("iquji-shared-agent")
                    .on_receive_notification(
                        async move |notification: SessionNotification, _cx| {
                            let mut state = updates.lock().map_err(|_| Error::internal_error())?;
                            if state
                                .session
                                .as_deref()
                                .is_some_and(|id| id != notification.session_id.to_string())
                            {
                                state.error = Some(AgentError::SessionMismatch);
                                return Err(Error::invalid_request());
                            }
                            // loadSession 重放的历史不冒充本回合流；只投影明确的 Agent 文本。
                            if !state.prompting || state.error.is_some() {
                                return Ok(());
                            }
                            if let SessionUpdate::AgentMessageChunk(chunk) = notification.update
                                && let ContentBlock::Text(content) = chunk.content
                            {
                                let text: String = content
                                    .text
                                    .chars()
                                    .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
                                    .collect();
                                state.text.push_str(&text);
                                drop(state);
                                events(TurnEvent::TextDelta { text });
                            }
                            Ok(())
                        },
                        agent_client_protocol::on_receive_notification!(),
                    )
                    .on_receive_request(
                        async move |_request: RequestPermissionRequest, responder, _cx| {
                            // Shared 不签发工具权限，未知 Agent 不能借会话请求提升宿主权限。
                            responder.respond(RequestPermissionResponse::new(
                                RequestPermissionOutcome::Cancelled,
                            ))
                        },
                        agent_client_protocol::on_receive_request!(),
                    )
                    .on_receive_request(
                        async |_request: ReadTextFileRequest, responder, _cx| {
                            responder.respond_with_error(Error::method_not_found())
                        },
                        agent_client_protocol::on_receive_request!(),
                    )
                    .on_receive_request(
                        async |_request: WriteTextFileRequest, responder, _cx| {
                            responder.respond_with_error(Error::method_not_found())
                        },
                        agent_client_protocol::on_receive_request!(),
                    )
                    .connect_with(streams, async move |connection| {
                        let initialized = connection
                            .send_request(InitializeRequest::new(ProtocolVersion::V1))
                            .block_task()
                            .await
                            .map_err(|error| remember_failure(&result, error))?;
                        if initialized.protocol_version != ProtocolVersion::V1 {
                            result.lock().map_err(|_| Error::internal_error())?.error =
                                Some(AgentError::ProtocolUnsupported);
                            return Ok(());
                        }
                        if !request.attachments.is_empty()
                            && !initialized.agent_capabilities.prompt_capabilities.image
                        {
                            result.lock().map_err(|_| Error::internal_error())?.error =
                                Some(AgentError::AttachmentUnsupported);
                            return Ok(());
                        }
                        if !request.mcp_servers.is_empty()
                            && !initialized.agent_capabilities.mcp_capabilities.http
                        {
                            result.lock().map_err(|_| Error::internal_error())?.error =
                                Some(AgentError::CapabilitiesUnsupported);
                            return Ok(());
                        }
                        let mcp_servers = request
                            .mcp_servers
                            .iter()
                            .map(|WireMcpServer { name, url }| {
                                McpServer::Http(McpServerHttp::new(name.clone(), url.clone()))
                            })
                            .collect::<Vec<_>>();
                        let images = match crate::attachments::load(
                            &request.attachments,
                            cancelled,
                            deadline,
                        ) {
                            Ok(images) => images,
                            Err(error) => {
                                result.lock().map_err(|_| Error::internal_error())?.error =
                                    Some(error);
                                return Ok(());
                            }
                        };
                        let mut prompt = vec![ContentBlock::Text(TextContent::new(request.prompt))];
                        for image in images {
                            if let Err(error) = crate::attachments::check(cancelled, deadline) {
                                result.lock().map_err(|_| Error::internal_error())?.error =
                                    Some(error);
                                return Ok(());
                            }
                            let data =
                                base64::engine::general_purpose::STANDARD.encode(&image.bytes);
                            if let Err(error) = crate::attachments::check(cancelled, deadline) {
                                result.lock().map_err(|_| Error::internal_error())?.error =
                                    Some(error);
                                return Ok(());
                            }
                            prompt.push(ContentBlock::Image(ImageContent::new(
                                data,
                                image.media_type(),
                            )));
                        }
                        let resumable = initialized.agent_capabilities.load_session;
                        let session = if let Some(session) = request.session_id {
                            if !resumable {
                                result.lock().map_err(|_| Error::internal_error())?.error =
                                    Some(AgentError::SessionUnsupported);
                                return Ok(());
                            }
                            result.lock().map_err(|_| Error::internal_error())?.session =
                                Some(session.clone());
                            connection
                                .send_request(
                                    LoadSessionRequest::new(session.clone(), cwd)
                                        .mcp_servers(mcp_servers),
                                )
                                .block_task()
                                .await
                                .map_err(|error| remember_failure(&result, error))?;
                            session
                        } else {
                            let created = connection
                                .send_request(NewSessionRequest::new(cwd).mcp_servers(mcp_servers))
                                .block_task()
                                .await
                                .map_err(|error| remember_failure(&result, error))?;
                            created.session_id.to_string()
                        };
                        if !valid_session_id(&session) {
                            result.lock().map_err(|_| Error::internal_error())?.error =
                                Some(AgentError::ProtocolInvalid);
                            return Ok(());
                        }
                        {
                            let mut state = result.lock().map_err(|_| Error::internal_error())?;
                            state.session = Some(session.clone());
                            state.prompting = true;
                        }
                        on_event(TurnEvent::SessionStarted {
                            session_id: session.clone(),
                            resumable,
                        });
                        let response = connection
                            .send_request(PromptRequest::new(session.clone(), prompt))
                            .block_task()
                            .await
                            .map_err(|error| remember_failure(&result, error))?;
                        let stop_reason = serde_json::to_value(response.stop_reason)
                            .ok()
                            .and_then(|value| value.as_str().map(str::to_owned))
                            .ok_or_else(Error::internal_error)?;
                        let mut state = result.lock().map_err(|_| Error::internal_error())?;
                        state.prompting = false;
                        state.result = Some(TurnResult {
                            session_id: Some(session),
                            resumable,
                            text: state.text.clone(),
                            stop_reason,
                        });
                        Ok(())
                    })
            },
        );
        output.map_err(|error| match error {
            AgentError::ProbeTimedOut => AgentError::TurnTimedOut,
            AgentError::ProbeCancelled => AgentError::TurnCancelled,
            AgentError::ProbeOutputLimit => AgentError::TurnOutputLimit,
            other => other,
        })?;
        let mut state = state.lock().map_err(|_| AgentError::ProcessIo)?;
        if let Some(error) = state.error {
            return Err(error);
        }
        state.result.take().ok_or(AgentError::ProtocolInvalid)
    }
}

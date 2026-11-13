use crate::{AgentError, Configuration};
use std::path::Path;
use std::sync::atomic::AtomicBool;

pub(crate) struct Capabilities {
    pub(crate) load_session: bool,
    pub(crate) images: bool,
}

pub(crate) fn inspect(
    program: &Path,
    configuration: &Configuration,
    cancelled: &AtomicBool,
) -> Result<Capabilities, AgentError> {
    #[cfg(unix)]
    {
        use agent_client_protocol::Client;
        use agent_client_protocol::schema::{ProtocolVersion, v1::InitializeRequest};
        use std::sync::{Arc, Mutex};
        let capabilities = Arc::new(Mutex::new(None));
        let received = capabilities.clone();
        crate::acp_transport::run(
            program,
            configuration,
            cancelled,
            std::time::Duration::from_millis(configuration.timeout_ms),
            configuration.max_output_bytes,
            || None,
            move |streams| {
                Client.builder().name("iquji-shared-agent").connect_with(
                    streams,
                    async move |connection| {
                        let response = connection
                            .send_request(InitializeRequest::new(ProtocolVersion::V1))
                            .block_task()
                            .await?;
                        let result = if response.protocol_version != ProtocolVersion::V1 {
                            Err(AgentError::ProtocolUnsupported)
                        } else {
                            Ok(Capabilities {
                                load_session: response.agent_capabilities.load_session,
                                images: response.agent_capabilities.prompt_capabilities.image,
                            })
                        };
                        *received
                            .lock()
                            .map_err(|_| agent_client_protocol::Error::internal_error())? =
                            Some(result);
                        Ok(())
                    },
                )
            },
        )?;
        capabilities
            .lock()
            .map_err(|_| AgentError::ProcessIo)?
            .take()
            .ok_or(AgentError::ProtocolInvalid)?
    }
    #[cfg(not(unix))]
    {
        let _ = (program, configuration, cancelled);
        Err(AgentError::PlatformUnsupported)
    }
}

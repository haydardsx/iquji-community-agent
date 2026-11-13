//! 通用 Agent 进程的唯一机器合同，不执行进程、读取配置或授予业务权限。

mod attachment;
mod configuration;
mod context;
mod error;
mod process_context;
mod scan;
mod turn;

pub use attachment::{Attachment, MAX_IMAGE_BYTES, MAX_IMAGES, validate_attachments};
pub use configuration::{
    ConfigurationInspection, InspectedAgent, ModelConfiguration, PreparedConfigurationFile,
    PreparedModelOverride, inspection_context_fingerprint, valid_provider_identifier,
};
pub use error::AgentError;
pub use process_context::{PROCESS_GROUP_OWNER_ARGUMENT, ProcessExecutionContext};
pub use scan::{Adapter, Connection, ScanReport};
pub use turn::{
    MAX_MCP_SERVER_NAME_BYTES, MAX_MCP_SERVER_URL_BYTES, MAX_MCP_SERVERS, MAX_PROMPT_BYTES,
    MAX_REQUEST_BYTES, MAX_TURN_OUTPUT_BYTES, McpServer, TurnEvent, TurnOutput, TurnRequest,
    TurnResult, TurnTerminal, decode_turn_output, decode_turn_request, valid_agent_id,
    valid_model_identifier, valid_session_id,
};

pub use context::turn_context_fingerprint;

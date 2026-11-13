use crate::{AgentError, Attachment, validate_attachments};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_PROMPT_BYTES: usize = 65_536;
pub const MAX_REQUEST_BYTES: usize = 400 * 1024;
pub const MAX_TURN_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_MCP_SERVERS: usize = 32;
pub const MAX_MCP_SERVER_NAME_BYTES: usize = 120;
pub const MAX_MCP_SERVER_URL_BYTES: usize = 256;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServer {
    pub name: String,
    pub url: String,
}

impl McpServer {
    pub fn valid(&self) -> bool {
        !self.name.is_empty()
            && self.name.len() <= MAX_MCP_SERVER_NAME_BYTES
            && self
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            && !self.url.is_empty()
            && self.url.len() <= MAX_MCP_SERVER_URL_BYTES
            && !self.url.chars().any(char::is_control)
            && url::Url::parse(&self.url).is_ok_and(|parsed| {
                parsed.scheme() == "http"
                    && parsed.host_str() == Some("127.0.0.1")
                    && parsed.port().is_some()
                    && parsed.username().is_empty()
                    && parsed.password().is_none()
                    && parsed.query().is_none()
                    && parsed.fragment().is_none()
                    && parsed.path().strip_prefix("/mcp/").is_some_and(|token| {
                        token.len() == 48 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
            })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TurnRequest {
    #[serde(default)]
    pub workspace_write: bool,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_fingerprint: Option<String>,
    pub prompt: String,
    pub session_id: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServer>,
}

impl TurnRequest {
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.context_fingerprint.as_ref().is_some_and(|value| {
            value.len() != 64
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) || self
            .model
            .as_ref()
            .is_some_and(|model| !valid_model_identifier(model))
            || !valid_agent_id(&self.agent_id)
            || self.prompt.trim().is_empty()
            || self.prompt.len() > MAX_PROMPT_BYTES
            || self
                .prompt
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\t' | '\r'))
            || self
                .session_id
                .as_ref()
                .is_some_and(|id| !valid_session_id(id))
            || !valid_mcp_servers(&self.mcp_servers)
        {
            return Err(AgentError::RequestInvalid);
        }
        validate_attachments(&self.attachments)
    }
}

fn valid_mcp_servers(servers: &[McpServer]) -> bool {
    if servers.len() > MAX_MCP_SERVERS {
        return false;
    }
    let mut names = HashSet::with_capacity(servers.len());
    servers
        .iter()
        .all(|server| server.valid() && names.insert(server.name.as_str()))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TurnEvent {
    SessionStarted { session_id: String, resumable: bool },
    TextDelta { text: String },
}

impl TurnEvent {
    pub fn validate(&self) -> Result<(), AgentError> {
        match self {
            Self::SessionStarted { session_id, .. } if !valid_session_id(session_id) => {
                Err(AgentError::ProtocolInvalid)
            }
            Self::TextDelta { text } if text.len() > MAX_TURN_OUTPUT_BYTES => {
                Err(AgentError::TurnOutputLimit)
            }
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TurnResult {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_session"
    )]
    pub session_id: Option<String>,
    pub resumable: bool,
    pub text: String,
    pub stop_reason: String,
}

impl TurnResult {
    pub fn validate(&self) -> Result<(), AgentError> {
        if self
            .session_id
            .as_ref()
            .is_some_and(|id| !valid_session_id(id))
            || (self.resumable && self.session_id.is_none())
            || self.stop_reason.is_empty()
            || self.stop_reason.len() > 64
            || self.stop_reason.chars().any(char::is_control)
        {
            return Err(AgentError::ProtocolInvalid);
        }
        if self.text.len() > MAX_TURN_OUTPUT_BYTES {
            return Err(AgentError::TurnOutputLimit);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TurnTerminal {
    TurnCompleted {
        #[serde(flatten)]
        result: TurnResult,
    },
    TurnError {
        error: AgentError,
    },
}

impl TurnTerminal {
    pub fn validate(&self) -> Result<(), AgentError> {
        match self {
            Self::TurnCompleted { result } => result.validate(),
            Self::TurnError { .. } => Ok(()),
        }
    }
}

// 事件与终态共用一条 NDJSON 通道；不复制同名字段或另设产品 DTO。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum TurnOutput {
    Event(TurnEvent),
    Terminal(TurnTerminal),
}

impl TurnOutput {
    pub fn validate(&self) -> Result<(), AgentError> {
        match self {
            Self::Event(event) => event.validate(),
            Self::Terminal(terminal) => terminal.validate(),
        }
    }
}

pub fn decode_turn_request(bytes: &[u8]) -> Result<TurnRequest, AgentError> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(AgentError::RequestInvalid);
    }
    let request: TurnRequest =
        serde_json::from_slice(bytes).map_err(|_| AgentError::RequestInvalid)?;
    request.validate()?;
    Ok(request)
}

/// 接收方仍须用总字节与墙钟预算读取管道，并核对唯一终态及进程退出。
pub fn decode_turn_output(bytes: &[u8]) -> Result<TurnOutput, AgentError> {
    if bytes.len() > MAX_TURN_OUTPUT_BYTES {
        return Err(AgentError::TurnOutputLimit);
    }
    let output: TurnOutput =
        serde_json::from_slice(bytes).map_err(|_| AgentError::ProtocolInvalid)?;
    output.validate()?;
    Ok(output)
}

pub fn valid_agent_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.as_bytes()[0].is_ascii_lowercase()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control)
}

fn present_session<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

pub fn valid_model_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':' | b'/'))
}

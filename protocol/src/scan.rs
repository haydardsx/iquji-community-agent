use crate::{AgentError, valid_agent_id};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Adapter {
    CodexCli,
    DeepseekHarness,
    NativeAcp,
    ExternalAcpAdapter,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScanReport {
    pub schema_version: u32,
    pub kind: String,
    pub readiness: String,
    pub connections: Vec<Connection>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Connection {
    pub id: String,
    pub adapter: Adapter,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable_sha256: Option<String>,
    pub status: String,
    pub version: Option<String>,
    pub protocol_version: Option<u16>,
    pub session_mode: Option<String>,
    pub supported_input_formats: Vec<String>,
    pub error: Option<AgentError>,
}

impl ScanReport {
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.schema_version != 1
            || self.kind != "agent_connection_scan"
            || self.readiness != "not_checked"
            || self.connections.is_empty()
            || self.connections.len() > 16
        {
            return Err(AgentError::ProtocolInvalid);
        }
        let mut ids = BTreeSet::new();
        for connection in &self.connections {
            connection.validate()?;
            if !ids.insert(&connection.id) {
                return Err(AgentError::ProtocolInvalid);
            }
        }
        Ok(())
    }
}

impl Connection {
    pub fn validate(&self) -> Result<(), AgentError> {
        if !valid_agent_id(&self.id)
            || self.context_fingerprint.is_some() != self.executable_sha256.is_some()
            || self
                .executable_sha256
                .as_ref()
                .is_some_and(|value| !crate::configuration::valid_fingerprint(value))
            || self
                .context_fingerprint
                .as_ref()
                .is_some_and(|value| !crate::configuration::valid_fingerprint(value))
            || (self.status == "unavailable" && self.context_fingerprint.is_some())
            || !matches!(self.status.as_str(), "detected" | "unavailable")
            || self
                .version
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control))
            || self.session_mode.as_ref().is_some_and(|s| {
                !matches!(
                    s.as_str(),
                    "resumable" | "connection_scoped" | "single_turn"
                )
            })
            || self.supported_input_formats.len() > 2
            || self
                .supported_input_formats
                .iter()
                .any(|s| !matches!(s.as_str(), "image/png" | "image/jpeg"))
            || self
                .supported_input_formats
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != self.supported_input_formats.len()
            || (self.status == "detected" && (self.error.is_some() || self.session_mode.is_none()))
            || (self.status == "unavailable"
                && (self.error.is_none()
                    || self.session_mode.is_some()
                    || !self.supported_input_formats.is_empty()))
        {
            return Err(AgentError::ProtocolInvalid);
        }
        Ok(())
    }
}

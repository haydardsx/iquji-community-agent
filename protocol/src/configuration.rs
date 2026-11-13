use crate::{Adapter, AgentError};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelConfiguration {
    pub provider: String,
    pub configured_model: Option<String>,
    pub supported_models: Vec<String>,
    pub credential_environment_variables: Vec<String>,
    pub configuration_fingerprint: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectedAgent {
    pub id: String,
    pub adapter: Adapter,
    pub inspection_context_fingerprint: Option<String>,
    pub metadata: Option<ModelConfiguration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prepared_model_override: Option<PreparedModelOverride>,
    pub error: Option<AgentError>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedConfigurationFile {
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedModelOverride {
    pub model: String,
    pub patch: PreparedConfigurationFile,
    pub settings: Option<PreparedConfigurationFile>,
}

impl PreparedModelOverride {
    pub fn validate(&self) -> Result<(), AgentError> {
        if !crate::valid_model_identifier(&self.model)
            || [&self.patch]
                .into_iter()
                .chain(self.settings.as_ref())
                .any(|file| {
                    file.size == 0 || file.size > 1_048_576 || !valid_fingerprint(&file.sha256)
                })
        {
            return Err(AgentError::ProtocolInvalid);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationInspection {
    pub schema_version: u32,
    pub kind: String,
    pub readiness: String,
    pub agents: Vec<InspectedAgent>,
}

impl ModelConfiguration {
    pub fn validate(&self) -> Result<(), AgentError> {
        if !valid_provider_identifier(&self.provider)
            || self
                .configured_model
                .as_ref()
                .is_some_and(|m| !crate::valid_model_identifier(m))
            || self.supported_models.len() > 512
            || self
                .supported_models
                .iter()
                .any(|m| !crate::valid_model_identifier(m))
            || self
                .supported_models
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.supported_models.len()
            || self.credential_environment_variables.len() > 32
            || self.credential_environment_variables.iter().any(|s| {
                s.is_empty()
                    || s.len() > 128
                    || !s.bytes().enumerate().all(|(i, b)| {
                        b.is_ascii_alphabetic() || b == b'_' || i > 0 && b.is_ascii_digit()
                    })
            })
            || self
                .credential_environment_variables
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.credential_environment_variables.len()
            || !valid_fingerprint(&self.configuration_fingerprint)
        {
            return Err(AgentError::ProtocolInvalid);
        }
        Ok(())
    }
}
impl ConfigurationInspection {
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.schema_version != 1
            || self.kind != "agent_configuration_inspection"
            || self.readiness != "not_checked"
            || self.agents.is_empty()
            || self.agents.len() > 16
        {
            return Err(AgentError::ProtocolInvalid);
        }
        let mut ids = std::collections::BTreeSet::new();
        for agent in &self.agents {
            if !crate::valid_agent_id(&agent.id)
                || !ids.insert(&agent.id)
                || agent.metadata.is_some() == agent.error.is_some()
                || agent
                    .inspection_context_fingerprint
                    .as_ref()
                    .is_some_and(|fingerprint| !valid_fingerprint(fingerprint))
                || agent.metadata.is_some() && agent.inspection_context_fingerprint.is_none()
            {
                return Err(AgentError::ProtocolInvalid);
            }
            if let Some(metadata) = &agent.metadata {
                metadata.validate()?;
            }
            if let Some(prepared) = &agent.prepared_model_override {
                if agent.adapter != Adapter::DeepseekHarness || agent.metadata.is_none() {
                    return Err(AgentError::ProtocolInvalid);
                }
                prepared.validate()?;
            }
        }
        Ok(())
    }
}

/// 将受检制品与配置目录绑定为不暴露本机路径的身份；调用方负责实际文件核验。
pub fn inspection_context_fingerprint(
    adapter: Adapter,
    executable_sha256: &str,
    canonical_configuration_directory: &str,
    working_directory: Option<&str>,
) -> Result<String, AgentError> {
    use sha2::{Digest, Sha256};
    if !valid_fingerprint(executable_sha256)
        || !valid_directory(canonical_configuration_directory)
        || (adapter == Adapter::DeepseekHarness) != working_directory.is_some()
        || working_directory.is_some_and(|value| !valid_directory(value))
    {
        return Err(AgentError::ProtocolInvalid);
    }
    let adapter = serde_json::to_vec(&adapter).map_err(|_| AgentError::ProtocolInvalid)?;
    let mut digest = Sha256::new();
    for field in [
        b"iquji-agent-inspection-context/v1".as_slice(),
        adapter.as_slice(),
        executable_sha256.as_bytes(),
        canonical_configuration_directory.as_bytes(),
    ] {
        digest.update((field.len() as u32).to_be_bytes());
        digest.update(field);
    }
    if let Some(directory) = working_directory {
        digest.update((directory.len() as u32).to_be_bytes());
        digest.update(directory.as_bytes());
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(crate) fn valid_fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn valid_provider_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

pub(crate) fn valid_directory(value: &str) -> bool {
    use std::path::{Component, Path, PathBuf};
    let directory = Path::new(value);
    let normalized: PathBuf = directory.components().collect();
    !value.is_empty()
        && value.len() <= 4096
        && !value.chars().any(char::is_control)
        && directory.is_absolute()
        && !directory
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        && normalized.to_str() == Some(value)
}

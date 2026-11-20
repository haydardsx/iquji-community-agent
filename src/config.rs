use crate::{Adapter, AgentError};
use iquji_agent_wire::valid_agent_id;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_CONFIG_BYTES: u64 = 32 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Agent {
    pub(crate) id: String,
    pub(crate) adapter: Adapter,
    pub(crate) executable: PathBuf,
    pub(crate) expected_sha256: Option<String>,
    pub(crate) codex_configuration_directory: Option<PathBuf>,
    pub(crate) deepseek_configuration_directory: Option<PathBuf>,
    #[serde(default)]
    pub(crate) environment_variables: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Configuration {
    #[serde(skip)]
    pub(crate) process_context: Option<iquji_agent_wire::ProcessExecutionContext>,
    schema_version: u32,
    pub(crate) working_directory: PathBuf,
    pub(crate) agents: Vec<Agent>,
    #[serde(default = "default_timeout")]
    pub(crate) timeout_ms: u64,
    #[serde(default = "default_output_limit")]
    pub(crate) max_output_bytes: usize,
    #[serde(default = "default_turn_timeout")]
    pub(crate) turn_timeout_ms: u64,
    #[serde(default = "default_turn_output_limit")]
    pub(crate) max_turn_output_bytes: usize,
}

const fn default_timeout() -> u64 {
    1_000
}
const fn default_output_limit() -> usize {
    16_384
}

const fn default_turn_timeout() -> u64 {
    60_000
}

const fn default_turn_output_limit() -> usize {
    1_048_576
}

impl Configuration {
    /// 外部进程组只由可信原生调用方在内存中装配，不能从配置 JSON 读取。
    pub fn with_process_context(
        mut self,
        context: iquji_agent_wire::ProcessExecutionContext,
    ) -> Result<Self, AgentError> {
        crate::process::validate_external_group(context)?;
        self.process_context = Some(context);
        Ok(self)
    }

    pub fn select_agent(&mut self, id: &str) -> Result<(), AgentError> {
        if !valid_agent_id(id) || !self.agents.iter().any(|agent| agent.id == id) {
            return Err(AgentError::RequestInvalid);
        }
        self.agents.retain(|agent| agent.id == id);
        Ok(())
    }

    pub fn inspection_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.timeout_ms * self.agents.len() as u64)
    }

    pub fn turn_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.turn_timeout_ms)
    }

    pub fn turn_output_limit(&self) -> usize {
        self.max_turn_output_bytes
    }

    pub fn load(path: &Path) -> Result<Self, AgentError> {
        if !valid_absolute_path(path) {
            return Err(AgentError::ConfigurationInvalid);
        }
        let metadata =
            std::fs::symlink_metadata(path).map_err(|_| AgentError::ConfigurationInvalid)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > MAX_CONFIG_BYTES
        {
            return Err(AgentError::ConfigurationInvalid);
        }
        private_configuration(path, &metadata)?;
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|_| AgentError::ConfigurationInvalid)?
            .take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| AgentError::ConfigurationInvalid)?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(AgentError::ConfigurationInvalid);
        }
        let config: Self =
            serde_json::from_slice(&bytes).map_err(|_| AgentError::ConfigurationInvalid)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), AgentError> {
        if self.schema_version != 1
            || self.agents.is_empty()
            || self.agents.len() > 16
            || !(10..=10_000).contains(&self.timeout_ms)
            || !(128..=65_536).contains(&self.max_output_bytes)
            || !(10..=600_000).contains(&self.turn_timeout_ms)
            || !(128..=4_194_304).contains(&self.max_turn_output_bytes)
            || !valid_absolute_path(&self.working_directory)
        {
            return Err(AgentError::ConfigurationInvalid);
        }
        let directory = std::fs::canonicalize(&self.working_directory)
            .map_err(|_| AgentError::ConfigurationInvalid)?;
        if directory != self.working_directory || !directory.is_dir() {
            return Err(AgentError::ConfigurationInvalid);
        }
        let mut identities = BTreeSet::new();
        for agent in &self.agents {
            if !valid_agent_id(&agent.id)
                || !identities.insert(&agent.id)
                || !valid_absolute_path(&agent.executable)
                || agent.expected_sha256.as_ref().is_some_and(|hash| {
                    hash.len() != 64
                        || !hash
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
            {
                return Err(AgentError::ConfigurationInvalid);
            }
            if (agent.adapter != Adapter::CodexCli && agent.codex_configuration_directory.is_some())
                || (agent.adapter != Adapter::DeepseekHarness
                    && agent.deepseek_configuration_directory.is_some())
                || (!matches!(agent.adapter, Adapter::CodexCli | Adapter::DeepseekHarness)
                    && !agent.environment_variables.is_empty())
                || agent.environment_variables.len() > 32
                || agent
                    .environment_variables
                    .iter()
                    .any(|name| !crate::cli_environment::valid_reference(name))
                || agent
                    .environment_variables
                    .iter()
                    .collect::<BTreeSet<_>>()
                    .len()
                    != agent.environment_variables.len()
            {
                return Err(AgentError::ConfigurationInvalid);
            }
            for directory in [
                &agent.codex_configuration_directory,
                &agent.deepseek_configuration_directory,
            ]
            .into_iter()
            .flatten()
            {
                if !valid_absolute_path(directory)
                    || std::fs::canonicalize(directory).ok().as_ref() != Some(directory)
                    || !directory.is_dir()
                {
                    return Err(AgentError::ConfigurationInvalid);
                }
            }
        }
        Ok(())
    }
}

fn valid_absolute_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .to_str()
            .is_some_and(|text| text.len() <= 4_096 && !text.chars().any(char::is_control))
        && !path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
}

#[cfg(unix)]
fn private_configuration(path: &Path, metadata: &std::fs::Metadata) -> Result<(), AgentError> {
    use std::os::unix::fs::MetadataExt;
    let parent = path.parent().ok_or(AgentError::ConfigurationInvalid)?;
    let parent_metadata =
        std::fs::symlink_metadata(parent).map_err(|_| AgentError::ConfigurationInvalid)?;
    // 本机可信配置不能由其他用户替换；网页请求也不能提供新的命令或参数。
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || !parent_metadata.is_dir()
        || parent_metadata.file_type().is_symlink()
        || parent_metadata.uid() != unsafe { libc::geteuid() }
        || parent_metadata.mode() & 0o022 != 0
    {
        return Err(AgentError::ConfigurationNotPrivate);
    }
    Ok(())
}

#[cfg(not(unix))]
fn private_configuration(_path: &Path, _metadata: &std::fs::Metadata) -> Result<(), AgentError> {
    Err(AgentError::PlatformUnsupported)
}

use crate::{Adapter, Agent, AgentError};
use std::collections::BTreeMap;
use std::ffi::OsString;

pub(crate) fn valid_reference(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    // 名称引用不能重定向配置搜索根或覆盖工具构造的基础环境。
    !name.is_empty()
        && name.len() <= 128
        && name.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphabetic() || byte == b'_' || index > 0 && byte.is_ascii_digit()
        })
        && ![
            "LD_",
            "DYLD_",
            "_DYLD_",
            "BASH_FUNC_",
            "GIT_",
            "RUST_",
            "CARGO_",
            "PYTHON",
            "PERL",
            "RUBY",
            "MALLOC_",
            "DSH_",
            "XDG_",
        ]
        .iter()
        .any(|prefix| upper.starts_with(prefix))
        && (!upper.starts_with("CODEX_") || upper == "CODEX_API_KEY")
        && ![
            "HOME",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
            "APPDATA",
            "LOCALAPPDATA",
            "ALLUSERSPROFILE",
            "CFFIXED_USER_HOME",
            "PATH",
            "LANG",
            "LC_ALL",
            "TERM",
            "TZ",
            "ENV",
            "BASH_ENV",
            "IFS",
            "SHELLOPTS",
            "BASHOPTS",
            "CDPATH",
            "GLOBIGNORE",
            "NODE_OPTIONS",
            "NODE_PATH",
            "JAVA_TOOL_OPTIONS",
            "_JAVA_OPTIONS",
            "JDK_JAVA_OPTIONS",
            "CLASSPATH",
            "GCONV_PATH",
            "LOCPATH",
            "NLSPATH",
            "GLIBC_TUNABLES",
            "SSLKEYLOGFILE",
            "STUDIO_AGENT_EXECUTABLE",
            "STUDIO_AGENT_CONFIGURATION",
            "STUDIO_AGENT_ARTIFACT_DIRECTORY",
        ]
        .contains(&upper.as_str())
}

pub(crate) struct CliEnvironment {
    directory: std::path::PathBuf,
    _temporary: Option<crate::private_directory::PrivateDirectory>,
    home_variable: &'static str,
}

impl CliEnvironment {
    pub(crate) fn new(agent: &Agent) -> Result<Self, AgentError> {
        let home_variable = home_variable(agent.adapter)?;
        let selected = match agent.adapter {
            Adapter::CodexCli => &agent.codex_configuration_directory,
            Adapter::DeepseekHarness => &agent.deepseek_configuration_directory,
            _ => return Err(AgentError::ConfigurationInvalid),
        };
        if let Some(directory) = selected {
            if std::fs::canonicalize(directory).ok().as_ref() != Some(directory)
                || !directory.is_dir()
            {
                return Err(AgentError::ConfigurationInvalid);
            }
            return Ok(Self {
                directory: directory.clone(),
                _temporary: None,
                home_variable,
            });
        }
        Self::isolated(agent.adapter)
    }

    pub(crate) fn isolated(adapter: Adapter) -> Result<Self, AgentError> {
        let home_variable = home_variable(adapter)?;
        let temporary = crate::private_directory::PrivateDirectory::new()?;
        Ok(Self {
            directory: temporary.path().to_path_buf(),
            _temporary: Some(temporary),
            home_variable,
        })
    }

    pub(crate) fn probe_values(&self) -> BTreeMap<String, OsString> {
        BTreeMap::from([
            (
                self.home_variable.to_owned(),
                self.directory.as_os_str().to_owned(),
            ),
            ("HOME".to_owned(), self.directory.as_os_str().to_owned()),
        ])
    }

    pub(crate) fn directory(&self) -> &std::path::Path {
        &self.directory
    }

    pub(crate) fn inspection_values(&self, default_home: &Self) -> BTreeMap<String, OsString> {
        let mut values = self.probe_values();
        values.insert(
            "HOME".to_owned(),
            default_home.directory.as_os_str().to_owned(),
        );
        if self.home_variable == "DSH_HOME" {
            values.insert("DSH_PERMISSION_MODE".into(), "read-only".into());
        }
        values
    }

    pub(crate) fn turn_values_with_references(
        &self,
        agent: &Agent,
        default_home: &Self,
        references: &[String],
    ) -> Result<BTreeMap<String, OsString>, AgentError> {
        let mut values = self.inspection_values(default_home);
        let names = agent
            .environment_variables
            .iter()
            .chain(references)
            .collect::<std::collections::BTreeSet<_>>();
        if names.len() > 32 {
            return Err(AgentError::ConfigurationInvalid);
        }
        let mut total = 0;
        for name in names {
            if !valid_reference(name) {
                return Err(AgentError::ConfigurationInvalid);
            }
            if let Some(value) = std::env::var_os(name) {
                let length = value.len();
                total += length;
                if length > 16_384 || total > 65_536 {
                    return Err(AgentError::ConfigurationInvalid);
                }
                values.insert(name.clone(), value);
            }
        }
        Ok(values)
    }
}

fn home_variable(adapter: Adapter) -> Result<&'static str, AgentError> {
    match adapter {
        Adapter::CodexCli => Ok("CODEX_HOME"),
        Adapter::DeepseekHarness => Ok("DSH_HOME"),
        _ => Err(AgentError::ConfigurationInvalid),
    }
}

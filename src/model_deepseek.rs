use crate::configuration_io::{MAX_CONFIGURATION_BYTES, read, read_if_present, referenced};
use crate::process_io::{Invocation, ProcessBudget};
use crate::{Agent, AgentError, Configuration};
use iquji_agent_wire::valid_model_identifier;
use serde_json::{Value, json};
use sha2::Digest;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub(crate) struct ParsedConfiguration {
    pub metadata: iquji_agent_wire::ModelConfiguration,
    model_entry_id: String,
    settings_entry: Option<(String, Value)>,
    settings: Value,
    entry_ids: BTreeSet<String>,
    mcp_server_names: BTreeSet<String>,
    stored_authentication_supported: bool,
}

#[cfg(unix)]
pub(crate) fn inspect(
    agent: &Agent,
    configuration: &Configuration,
    cancelled: &AtomicBool,
    budget: &mut ProcessBudget,
    environment: &std::collections::BTreeMap<String, std::ffi::OsString>,
) -> Result<ParsedConfiguration, AgentError> {
    let directory = agent
        .deepseek_configuration_directory
        .as_deref()
        .or_else(|| environment.get("DSH_HOME").map(std::path::Path::new))
        .ok_or(AgentError::ConfigurationMetadataUnavailable)?;
    let program =
        crate::configuration_inspection::turn_executable(agent, cancelled, budget.deadline)?;
    let mut output = Vec::new();
    let saved_deadline = budget.deadline;
    budget.deadline = budget.deadline.min(
        std::time::Instant::now() + std::time::Duration::from_millis(configuration.timeout_ms),
    );
    let result = crate::process_io::run(
        Invocation {
            program: &program,
            arguments: &["--profile", "headless", "--dump-config"],
            input: None,
            environment,
            output_limit: MAX_CONFIGURATION_BYTES,
        },
        configuration,
        cancelled,
        budget,
        |bytes| {
            output.extend_from_slice(bytes);
            Ok(())
        },
    );
    budget.deadline = saved_deadline;
    result?;
    let entries: serde_yaml_ng::Value =
        serde_yaml_ng::from_slice(&output).map_err(|_| AgentError::ConfigurationMetadataInvalid)?;
    let settings_entry = find_entry(&entries, "@deepseek-ai/dsh-settings-file");
    let settings = match settings_entry {
        Some(entry) => {
            let selected = entry_configuration(entry)?;
            let path = if let Some(path) = selected.get("path") {
                referenced(
                    &configuration.working_directory,
                    path.as_str()
                        .ok_or(AgentError::ConfigurationMetadataInvalid)?,
                )?
            } else if let Some(home) = selected.get("dshHome") {
                referenced(
                    &configuration.working_directory,
                    home.as_str()
                        .ok_or(AgentError::ConfigurationMetadataInvalid)?,
                )?
                .join("settings.yaml")
            } else {
                directory.join("settings.yaml")
            };
            let bytes = if selected.get("path").is_none() && selected.get("dshHome").is_none() {
                // 官方 settings provider 将缺失的默认文档视为空设置，不读取迁移备份。
                read_if_present(&path, cancelled, budget.deadline)?.unwrap_or_default()
            } else {
                read(&path, cancelled, budget.deadline)?
            };
            if bytes.iter().all(u8::is_ascii_whitespace) {
                json!({})
            } else {
                serde_yaml_ng::from_slice(&bytes)
                    .map_err(|_| AgentError::ConfigurationMetadataInvalid)?
            }
        }
        None => json!({}),
    };
    if !settings.is_object()
        || settings
            .get("agent-default-model")
            .is_some_and(|v| !v.is_object())
    {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    let parsed = from_sources(&entries, settings)?;
    if parsed.metadata.supported_models.len() > 512
        || parsed
            .metadata
            .credential_environment_variables
            .iter()
            .any(|name| !crate::cli_environment::valid_reference(name))
    {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    crate::configuration_io::check(cancelled, budget.deadline)?;
    Ok(parsed)
}

fn find_entry<'a>(
    entries: &'a serde_yaml_ng::Value,
    name: &str,
) -> Option<&'a serde_yaml_ng::Value> {
    for entry in entries.as_sequence()? {
        if entry
            .get("disabled")
            .and_then(serde_yaml_ng::Value::as_bool)
            == Some(true)
        {
            continue;
        }
        if entry.get("name").and_then(serde_yaml_ng::Value::as_str) == Some(name) {
            return Some(entry);
        }
        if entry.get("group").and_then(serde_yaml_ng::Value::as_bool) == Some(true)
            && let Some(found) = entry
                .get("config")
                .and_then(|children| find_entry(children, name))
        {
            return Some(found);
        }
    }
    None
}

fn entry_configuration(entry: &serde_yaml_ng::Value) -> Result<Value, AgentError> {
    entry.get("config").map_or_else(
        || Ok(json!({})),
        |value| serde_json::to_value(value).map_err(|_| AgentError::ConfigurationMetadataInvalid),
    )
}

fn from_sources(
    entries: &serde_yaml_ng::Value,
    settings: Value,
) -> Result<ParsedConfiguration, AgentError> {
    let mut fingerprint = sha2::Sha256::new();
    fingerprint.update(
        serde_yaml_ng::to_string(entries)
            .map_err(|_| AgentError::ConfigurationMetadataInvalid)?
            .as_bytes(),
    );
    fingerprint.update(
        serde_json::to_vec(&settings).map_err(|_| AgentError::ConfigurationMetadataInvalid)?,
    );
    let configuration_fingerprint = fingerprint
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let entry = find_entry(entries, "@deepseek-ai/dsh-agent-default-model")
        .ok_or(AgentError::ConfigurationMetadataInvalid)?;
    let selection = entry_configuration(entry)?;
    let stored = settings.get("agent-default-model");
    let setting = |key| {
        stored
            .and_then(|value| value.get(key))
            .or_else(|| selection.get(key))
    };
    let provider = setting("provider")
        .and_then(Value::as_str)
        .filter(|value| iquji_agent_wire::valid_provider_identifier(value))
        .ok_or(AgentError::ConfigurationMetadataInvalid)?
        .to_owned();
    let configured_model = setting("model")
        .and_then(Value::as_str)
        .filter(|value| valid_model_identifier(value))
        .map(str::to_owned);
    let mut supported_models: Vec<String> = configured_model.iter().cloned().collect();
    let adapter = if provider == "deepseek-official" {
        let base = find_entry(entries, "@deepseek-ai/dsh-llm-deepseek-api-key")
            .map(entry_configuration)
            .transpose()?
            .unwrap_or_else(|| json!({}));
        let mut effective = base;
        if let Some(values) = settings.get("llm-deepseek").and_then(Value::as_object)
            && let Some(base) = effective.as_object_mut()
        {
            base.extend(values.clone());
        }
        effective
    } else {
        let base = find_entry(entries, "@deepseek-ai/dsh-llm-pi-ai")
            .map(entry_configuration)
            .transpose()?
            .unwrap_or_else(|| json!({}));
        settings
            .get("llm-pi-ai")
            .and_then(|value| value.get("providers"))
            .or_else(|| base.get("providers"))
            .and_then(|value| value.get(&provider))
            .cloned()
            .unwrap_or_else(|| json!({}))
    };
    if let Some(models) = adapter.get("models").and_then(Value::as_array) {
        for model in models {
            if let Some(id) = model
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| valid_model_identifier(id))
                && !supported_models.iter().any(|model| model == id)
            {
                supported_models.push(id.to_owned());
                if supported_models.len() > 512 {
                    return Err(AgentError::ConfigurationMetadataInvalid);
                }
            }
        }
    }
    let mut credential_environment_variables = adapter
        .get("apiKeyEnv")
        .and_then(Value::as_str)
        .map(|name| vec![name.to_owned()])
        .unwrap_or_default();
    if provider == "deepseek-official" && credential_environment_variables.is_empty() {
        credential_environment_variables.push("DEEPSEEK_API_KEY".into());
    }
    let model_entry_id = entry
        .get("id")
        .and_then(serde_yaml_ng::Value::as_str)
        .ok_or(AgentError::ConfigurationMetadataInvalid)?
        .to_owned();
    let settings_entry = find_entry(entries, "@deepseek-ai/dsh-settings-file")
        .map(|entry| {
            let id = entry
                .get("id")
                .and_then(serde_yaml_ng::Value::as_str)
                .ok_or(AgentError::ConfigurationMetadataInvalid)?;
            Ok((id.to_owned(), entry_configuration(entry)?))
        })
        .transpose()?;
    let mut entry_ids = BTreeSet::new();
    let mut mcp_server_names = BTreeSet::new();
    inventory_entries(entries, &mut entry_ids, &mut mcp_server_names);
    Ok(ParsedConfiguration {
        metadata: iquji_agent_wire::ModelConfiguration {
            configured_model,
            supported_models,
            credential_environment_variables,
            configuration_fingerprint,
            provider,
        },
        model_entry_id,
        settings_entry,
        settings,
        entry_ids,
        mcp_server_names,
        stored_authentication_supported: stored_authentication_supported(entries),
    })
}

fn stored_authentication_supported(entries: &serde_yaml_ng::Value) -> bool {
    fn matches(entries: &serde_yaml_ng::Value, id: &str, name: &str) -> (usize, bool) {
        let mut count = 0;
        let mut valid = true;
        for entry in entries.as_sequence().into_iter().flatten() {
            if entry.get("id").and_then(serde_yaml_ng::Value::as_str) == Some(id) {
                count += 1;
                valid &= entry.get("name").and_then(serde_yaml_ng::Value::as_str) == Some(name);
            }
            if entry.get("group").and_then(serde_yaml_ng::Value::as_bool) == Some(true) {
                let (nested, safe) = matches(
                    entry.get("config").unwrap_or(&serde_yaml_ng::Value::Null),
                    id,
                    name,
                );
                count += nested;
                valid &= safe;
            }
        }
        (count, valid)
    }
    [
        ("credentials", "@deepseek-ai/dsh-credentials-local"),
        ("authorization", "@deepseek-ai/dsh-authorization"),
        ("deepseek-account", "@deepseek-ai/dsh-deepseek-account"),
    ]
    .iter()
    .all(|(id, name)| matches(entries, id, name) == (1, true))
}

fn inventory_entries(
    entries: &serde_yaml_ng::Value,
    ids: &mut BTreeSet<String>,
    mcp_server_names: &mut BTreeSet<String>,
) {
    inventory_entries_in_scope(entries, ids, mcp_server_names, true);
}

fn inventory_entries_in_scope(
    entries: &serde_yaml_ng::Value,
    ids: &mut BTreeSet<String>,
    mcp_server_names: &mut BTreeSet<String>,
    active: bool,
) {
    let Some(entries) = entries.as_sequence() else {
        return;
    };
    for entry in entries {
        if let Some(id) = entry.get("id").and_then(serde_yaml_ng::Value::as_str) {
            ids.insert(id.to_owned());
        }
        let entry_active = active
            && entry
                .get("disabled")
                .and_then(serde_yaml_ng::Value::as_bool)
                != Some(true);
        if entry_active
            && entry.get("name").and_then(serde_yaml_ng::Value::as_str)
                == Some("@deepseek-ai/dsh-mcp-client")
            && let Some(server_name) = entry
                .get("config")
                .and_then(|config| config.get("serverName"))
                .and_then(serde_yaml_ng::Value::as_str)
        {
            mcp_server_names.insert(server_name.to_owned());
        }
        if entry.get("group").and_then(serde_yaml_ng::Value::as_bool) == Some(true) {
            inventory_entries_in_scope(
                entry.get("config").unwrap_or(&serde_yaml_ng::Value::Null),
                ids,
                mcp_server_names,
                entry_active,
            );
        }
    }
}

pub(crate) struct ModelOverride {
    _root: crate::private_directory::PrivateDirectory,
    pub patch: PathBuf,
    files: Vec<(String, iquji_agent_wire::PreparedConfigurationFile)>,
}
impl ModelOverride {
    pub(crate) fn create(
        configuration: ParsedConfiguration,
        model: Option<&str>,
        mcp_servers: &[iquji_agent_wire::McpServer],
        restore_stored_authentication: bool,
        cancelled: &AtomicBool,
        deadline: std::time::Instant,
    ) -> Result<Self, AgentError> {
        if model.is_some_and(|model| !valid_model_identifier(model)) {
            return Err(AgentError::RequestInvalid);
        }
        let root = crate::private_directory::PrivateDirectory::new()?;
        let has_settings = write_runtime_override(
            configuration,
            model,
            mcp_servers,
            restore_stored_authentication,
            root.path(),
        )?;
        let mut files = vec![(
            "model.patch.json".to_owned(),
            crate::model_artifact::describe_file(
                root.path(),
                "model.patch.json",
                cancelled,
                deadline,
            )?,
        )];
        if has_settings {
            files.push((
                "settings.json".to_owned(),
                crate::model_artifact::describe_file(
                    root.path(),
                    "settings.json",
                    cancelled,
                    deadline,
                )?,
            ));
        }
        let result = Self {
            files,
            patch: root.path().join("model.patch.json"),
            _root: root,
        };
        result.verify(cancelled, deadline)?;
        Ok(result)
    }
    pub(crate) fn verify(
        &self,
        cancelled: &AtomicBool,
        deadline: std::time::Instant,
    ) -> Result<(), AgentError> {
        self._root.verify()?;
        let expected = self
            .files
            .iter()
            .map(|(name, descriptor)| (name.as_str(), descriptor))
            .collect::<Vec<_>>();
        crate::model_artifact::verify_named_files(
            self._root.path(),
            &expected,
            cancelled,
            deadline,
        )?;
        self._root.verify()
    }
}

pub(crate) fn write_override(
    configuration: ParsedConfiguration,
    model: &str,
    directory: &Path,
) -> Result<bool, AgentError> {
    if !valid_model_identifier(model) {
        return Err(AgentError::RequestInvalid);
    }
    write_runtime_override(configuration, Some(model), &[], false, directory)
}

fn write_runtime_override(
    mut configuration: ParsedConfiguration,
    model: Option<&str>,
    mcp_servers: &[iquji_agent_wire::McpServer],
    restore_stored_authentication: bool,
    directory: &Path,
) -> Result<bool, AgentError> {
    if model.is_none() && mcp_servers.is_empty() && !restore_stored_authentication {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    let mut patches = Vec::new();
    if restore_stored_authentication {
        if !configuration.stored_authentication_supported {
            return Err(AgentError::ConfigurationMetadataInvalid);
        }
        // 正式 headless 覆盖只恢复已有存储认证，由 CLI 自行解析，保持只读权限与用户型号。
        for id in ["credentials", "authorization", "deepseek-account"] {
            patches.push(json!({ "id": id, "disabled": false }));
        }
    }
    let mut has_settings = false;
    let selection =
        model.map(|model| json!({ "provider": configuration.metadata.provider, "model": model }));
    if let Some(selection) = &selection {
        patches.push(json!({ "id": configuration.model_entry_id, "config": selection }));
        has_settings = configuration.settings_entry.is_some();
    }
    if let Some(model) = model
        && let Some((id, mut config)) = configuration.settings_entry.take()
    {
        let selection = selection
            .as_ref()
            .ok_or(AgentError::ConfigurationMetadataInvalid)?;
        // settings 会覆盖组合层；复制原文档的值并只改本回合型号，避免改写用户偏好。
        let document = configuration
            .settings
            .as_object_mut()
            .ok_or(AgentError::ConfigurationMetadataInvalid)?;
        let saved = document
            .entry("agent-default-model")
            .or_insert(selection.clone());
        let saved = saved
            .as_object_mut()
            .ok_or(AgentError::ConfigurationMetadataInvalid)?;
        saved.insert("model".into(), Value::String(model.into()));
        saved.insert("provider".into(), selection["provider"].clone());
        let settings_path = directory.join("settings.json");
        write_private_document(&settings_path, &configuration.settings)?;
        let config = config
            .as_object_mut()
            .ok_or(AgentError::ConfigurationMetadataInvalid)?;
        config.insert(
            "path".into(),
            Value::String(settings_path.to_string_lossy().into_owned()),
        );
        patches.push(json!({ "id": id, "config": config }));
    }
    for server in mcp_servers {
        if !server.valid() {
            return Err(AgentError::RequestInvalid);
        }
        let id = format!("runtime-mcp-{}", server.name);
        if configuration.entry_ids.contains(&id)
            || configuration.mcp_server_names.contains(&server.name)
        {
            return Err(AgentError::ConfigurationMetadataInvalid);
        }
        patches.push(json!({
            "insert": [{
                "id": id,
                "name": "@deepseek-ai/dsh-mcp-client",
                "config": {
                    "serverName": server.name,
                    "transport": "streamable-http",
                    "url": server.url,
                    "headers": {}
                }
            }]
        }));
    }
    write_private_document(&directory.join("model.patch.json"), &Value::Array(patches))?;
    Ok(has_settings)
}

fn write_private_document(path: &Path, value: &Value) -> Result<(), AgentError> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).map_err(|_| AgentError::ProcessIo)?;
    let bytes = serde_json::to_vec(value).map_err(|_| AgentError::ConfigurationMetadataInvalid)?;
    if bytes.len() > MAX_CONFIGURATION_BYTES {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    file.write_all(&bytes).map_err(|_| AgentError::ProcessIo)
}

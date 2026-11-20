use crate::configuration_io::{read, referenced};
use crate::{Agent, AgentError};
use iquji_agent_wire::{ModelConfiguration, valid_model_identifier};
use sha2::{Digest, Sha256};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

pub(crate) fn inspect(
    agent: &Agent,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<ModelConfiguration, AgentError> {
    let directory = agent
        .codex_configuration_directory
        .as_ref()
        .ok_or(AgentError::ConfigurationMetadataUnavailable)?;
    let bytes = read(&directory.join("config.toml"), cancelled, deadline)?;
    let document: toml::Table = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(AgentError::ConfigurationMetadataInvalid)?;
    for name in ["model_provider", "model", "model_catalog_json"] {
        if document.get(name).is_some_and(|value| !value.is_str()) {
            return Err(AgentError::ConfigurationMetadataInvalid);
        }
    }
    let provider = document
        .get("model_provider")
        .and_then(toml::Value::as_str)
        .unwrap_or("openai");
    if !iquji_agent_wire::valid_provider_identifier(provider) {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    let adapter = document
        .get("model_providers")
        .and_then(|p| p.get(provider));
    if document
        .get("model_providers")
        .is_some_and(|v| !v.is_table())
        || adapter.is_some_and(|v| !v.is_table())
    {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    if adapter
        .and_then(|p| p.get("env_key"))
        .is_some_and(|v| !v.is_str())
        || adapter
            .and_then(|p| p.get("env_http_headers"))
            .is_some_and(|v| {
                !v.is_table() || v.as_table().unwrap().values().any(|value| !value.is_str())
            })
    {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    let mut refs = Vec::new();
    if let Some(name) = adapter
        .and_then(|p| p.get("env_key"))
        .and_then(toml::Value::as_str)
    {
        refs.push(name.to_owned());
    }
    if let Some(headers) = adapter
        .and_then(|p| p.get("env_http_headers"))
        .and_then(toml::Value::as_table)
    {
        refs.extend(
            headers
                .values()
                .filter_map(toml::Value::as_str)
                .map(str::to_owned),
        );
    }
    refs.sort();
    refs.dedup();
    if refs.len() > 32
        || refs
            .iter()
            .any(|name| !crate::cli_environment::valid_reference(name))
    {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    let configured_model = document
        .get("model")
        .and_then(toml::Value::as_str)
        .map(str::to_owned);
    if configured_model
        .as_ref()
        .is_some_and(|s| !valid_model_identifier(s))
    {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    let mut supported_models: Vec<String> = configured_model.iter().cloned().collect();
    let custom = provider != "openai"
        || document.contains_key("openai_base_url")
        || adapter.and_then(|p| p.get("base_url")).is_some();
    let catalog = if let Some(value) = document.get("model_catalog_json") {
        let path = referenced(
            directory,
            value
                .as_str()
                .ok_or(AgentError::ConfigurationMetadataInvalid)?,
        )?;
        Some(read(&path, cancelled, deadline)?)
    } else if !custom && directory.join("models_cache.json").exists() {
        Some(read(
            &directory.join("models_cache.json"),
            cancelled,
            deadline,
        )?)
    } else {
        None
    };
    if let Some(bytes) = catalog {
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| AgentError::ConfigurationMetadataInvalid)?;
        let entries = value
            .get("models")
            .and_then(serde_json::Value::as_array)
            .ok_or(AgentError::ConfigurationMetadataInvalid)?;
        for entry in entries {
            crate::configuration_io::check(cancelled, deadline)?;
            if entry
                .get("visibility")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|s| s != "list")
            {
                continue;
            }
            if let Some(model) = entry
                .get("slug")
                .and_then(serde_json::Value::as_str)
                .filter(|s| valid_model_identifier(s))
                && !supported_models.iter().any(|s| s == model)
            {
                supported_models.push(model.to_owned());
                if supported_models.len() > 512 {
                    return Err(AgentError::ConfigurationMetadataInvalid);
                }
            }
        }
    }
    if supported_models.len() > 512 {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    // 指纹涵盖配置及实际型号目录；不会把配置正文或提供方地址放入结果。
    let mut fingerprint = Sha256::new();
    fingerprint.update(
        serde_json::to_vec(&document).map_err(|_| AgentError::ConfigurationMetadataInvalid)?,
    );
    fingerprint.update(
        serde_json::to_vec(&supported_models)
            .map_err(|_| AgentError::ConfigurationMetadataInvalid)?,
    );
    let configuration_fingerprint = fingerprint
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    crate::configuration_io::check(cancelled, deadline)?;
    Ok(ModelConfiguration {
        provider: provider.into(),
        configured_model,
        supported_models,
        credential_environment_variables: refs,
        configuration_fingerprint,
    })
}

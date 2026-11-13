use crate::configuration_io::{MAX_CONFIGURATION_BYTES, read};
use crate::{AgentError, Configuration};
use iquji_agent_wire::McpServer;
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

pub(crate) fn prepare_home(
    source: &Path,
    configuration: &Configuration,
    servers: &[McpServer],
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<crate::private_directory::PrivateDirectory, AgentError> {
    if servers.is_empty() {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    let names = servers
        .iter()
        .map(|server| server.name.as_str())
        .collect::<HashSet<_>>();
    for directory in configuration.working_directory.ancestors() {
        crate::configuration_io::check(cancelled, deadline)?;
        let path = directory.join(".codex").join("config.toml");
        if let Some(document) = read_optional_config(&path, cancelled, deadline)? {
            reject_name_conflicts(&document, &names)?;
        }
    }

    let config_path = source.join("config.toml");
    let mut document = read_optional_config(&config_path, cancelled, deadline)?.unwrap_or_default();
    let configured_servers = document
        .entry("mcp_servers")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let configured_servers = configured_servers
        .as_table_mut()
        .ok_or(AgentError::ConfigurationMetadataInvalid)?;
    if names
        .iter()
        .any(|name| configured_servers.contains_key(*name))
    {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    for server in servers {
        crate::configuration_io::check(cancelled, deadline)?;
        let mut entry = toml::Table::new();
        entry.insert("url".into(), toml::Value::String(server.url.clone()));
        configured_servers.insert(server.name.clone(), toml::Value::Table(entry));
    }
    let source_bytes = toml::to_string(&toml::Value::Table(document))
        .map_err(|_| AgentError::ConfigurationMetadataInvalid)?;
    if source_bytes.len() > MAX_CONFIGURATION_BYTES {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }

    let mut temporary = crate::private_directory::PrivateDirectory::new()?;
    let mut entries = std::fs::read_dir(source)
        .map_err(|_| AgentError::ConfigurationMetadataUnavailable)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AgentError::ConfigurationMetadataUnavailable)?;
    if entries.len() > 4096 {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    entries.sort();
    for name in entries {
        crate::configuration_io::check(cancelled, deadline)?;
        if name == "config.toml" {
            continue;
        }
        temporary.register_cleanup_entry(&name)?;
        let destination = temporary.path().join(&name);
        let target = source.join(&name);
        std::os::unix::fs::symlink(target, destination)
            .map_err(|_| AgentError::ConfigurationMetadataUnavailable)?;
    }
    crate::configuration_io::check(cancelled, deadline)?;
    write_private_config(
        &temporary.path().join("config.toml"),
        source_bytes.as_bytes(),
    )?;
    temporary.verify()?;
    Ok(temporary)
}

fn read_optional_config(
    path: &Path,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<Option<toml::Table>, AgentError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            let bytes = read(path, cancelled, deadline)?;
            let document = std::str::from_utf8(&bytes)
                .ok()
                .and_then(|source| source.parse().ok())
                .ok_or(AgentError::ConfigurationMetadataInvalid)?;
            Ok(Some(document))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(AgentError::ConfigurationMetadataUnavailable),
    }
}

fn reject_name_conflicts(document: &toml::Table, names: &HashSet<&str>) -> Result<(), AgentError> {
    let servers = document.get("mcp_servers");
    if servers.is_some_and(|value| !value.is_table()) {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    if servers
        .and_then(toml::Value::as_table)
        .is_some_and(|servers| names.iter().any(|name| servers.contains_key(*name)))
    {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    Ok(())
}

fn write_private_config(path: &Path, bytes: &[u8]) -> Result<(), AgentError> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(path)
        .map_err(|_| AgentError::ConfigurationMetadataUnavailable)?;
    file.write_all(bytes)
        .map_err(|_| AgentError::ConfigurationMetadataUnavailable)?;
    file.sync_all()
        .map_err(|_| AgentError::ConfigurationMetadataUnavailable)
}

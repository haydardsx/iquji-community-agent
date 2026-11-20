use crate::AgentError;
use iquji_agent_wire::{PreparedConfigurationFile, PreparedModelOverride};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

pub(crate) fn validate_directory(directory: &Path) -> Result<(), AgentError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(directory)
            .map_err(|_| AgentError::ConfigurationMetadataInvalid)?;
        if !directory.is_absolute()
            || std::fs::canonicalize(directory).ok().as_deref() != Some(directory)
            || !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != 0o700
        {
            return Err(AgentError::ConfigurationNotPrivate);
        }
        if std::fs::read_dir(directory)
            .map_err(|_| AgentError::ConfigurationMetadataInvalid)?
            .next()
            .is_some()
        {
            return Err(AgentError::ConfigurationMetadataInvalid);
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
        Err(AgentError::PlatformUnsupported)
    }
}

pub(crate) fn prepare(
    configuration: crate::model_deepseek::ParsedConfiguration,
    model: &str,
    directory: &Path,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<PreparedModelOverride, AgentError> {
    validate_directory(directory)?;
    crate::configuration_io::check(cancelled, deadline)?;
    let has_settings = crate::model_deepseek::write_override(configuration, model, directory)?;
    let prepared = PreparedModelOverride {
        model: model.into(),
        patch: describe_file(directory, "model.patch.json", cancelled, deadline)?,
        settings: if has_settings {
            Some(describe_file(
                directory,
                "settings.json",
                cancelled,
                deadline,
            )?)
        } else {
            None
        },
    };
    prepared.validate()?;
    Ok(prepared)
}

pub(crate) fn describe_file(
    directory: &Path,
    name: &str,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<PreparedConfigurationFile, AgentError> {
    let bytes = crate::configuration_io::read(&directory.join(name), cancelled, deadline)?;
    Ok(PreparedConfigurationFile {
        size: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    })
}

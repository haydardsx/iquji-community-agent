use crate::AgentError;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

pub(crate) const MAX_CONFIGURATION_BYTES: usize = 1_048_576;

pub(crate) fn check(cancelled: &AtomicBool, deadline: Instant) -> Result<(), AgentError> {
    if cancelled.load(Ordering::SeqCst) {
        return Err(AgentError::ProbeCancelled);
    }
    if Instant::now() >= deadline {
        return Err(AgentError::ProbeTimedOut);
    }
    Ok(())
}

pub(crate) fn referenced(base: &Path, value: &str) -> Result<PathBuf, AgentError> {
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    let joined = base.join(value);
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(AgentError::ConfigurationMetadataInvalid);
                }
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

pub(crate) fn read(
    path: &Path,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<Vec<u8>, AgentError> {
    read_if_present(path, cancelled, deadline)?.ok_or(AgentError::ConfigurationMetadataUnavailable)
}

pub(crate) fn read_if_present(
    path: &Path,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<Option<Vec<u8>>, AgentError> {
    check(cancelled, deadline)?;
    // 可信配置只授权其元数据引用，不读取 CLI 登录凭据文件。
    if path
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("auth.json"))
    {
        return Err(AgentError::ConfigurationMetadataUnavailable);
    }
    let mut missing = false;
    for (index, ancestor) in path.ancestors().enumerate() {
        let metadata = match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) => metadata,
            Err(error) if index == 0 && error.kind() == std::io::ErrorKind::NotFound => {
                missing = true;
                continue;
            }
            Err(_) => return Err(AgentError::ConfigurationMetadataUnavailable),
        };
        if metadata.file_type().is_symlink() {
            return Err(AgentError::ConfigurationMetadataUnavailable);
        }
    }
    if missing {
        let parent = path
            .parent()
            .ok_or(AgentError::ConfigurationMetadataUnavailable)?;
        if std::fs::canonicalize(parent).ok().as_deref() != Some(parent) {
            return Err(AgentError::ConfigurationMetadataUnavailable);
        }
        check(cancelled, deadline)?;
        return Ok(None);
    }
    if std::fs::canonicalize(path).ok().as_deref() != Some(path) {
        return Err(AgentError::ConfigurationMetadataUnavailable);
    }
    let before = std::fs::symlink_metadata(path)
        .map_err(|_| AgentError::ConfigurationMetadataUnavailable)?;
    validate(&before)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options
        .open(path)
        .map_err(|_| AgentError::ConfigurationMetadataUnavailable)?;
    if std::fs::canonicalize(path).ok().as_deref() != Some(path) {
        return Err(AgentError::ConfigurationMetadataUnavailable);
    }
    let opened = file
        .metadata()
        .map_err(|_| AgentError::ConfigurationMetadataUnavailable)?;
    validate(&opened)?;
    if identity(&before) != identity(&opened) {
        return Err(AgentError::ConfigurationMetadataUnavailable);
    }
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        check(cancelled, deadline)?;
        let count = file
            .read(&mut buffer)
            .map_err(|_| AgentError::ConfigurationMetadataUnavailable)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.len() > MAX_CONFIGURATION_BYTES {
            return Err(AgentError::ConfigurationMetadataUnavailable);
        }
    }
    if std::fs::canonicalize(path).ok().as_deref() != Some(path) {
        return Err(AgentError::ConfigurationMetadataUnavailable);
    }
    let after = std::fs::symlink_metadata(path)
        .map_err(|_| AgentError::ConfigurationMetadataUnavailable)?;
    validate(&after)?;
    if identity(&before) != identity(&after)
        || identity(&before)
            != identity(
                &file
                    .metadata()
                    .map_err(|_| AgentError::ConfigurationMetadataUnavailable)?,
            )
    {
        return Err(AgentError::ConfigurationMetadataUnavailable);
    }
    Ok(Some(bytes))
}

fn validate(metadata: &std::fs::Metadata) -> Result<(), AgentError> {
    if !metadata.is_file() || metadata.len() > MAX_CONFIGURATION_BYTES as u64 {
        return Err(AgentError::ConfigurationMetadataUnavailable);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
            return Err(AgentError::ConfigurationMetadataUnavailable);
        }
    }
    Ok(())
}

fn identity(metadata: &std::fs::Metadata) -> (u64, u64, u64, u32, u128, u128) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mode(),
            metadata.mtime_nsec() as u128 ^ ((metadata.mtime() as u128) << 64),
            metadata.ctime_nsec() as u128 ^ ((metadata.ctime() as u128) << 64),
        )
    }
    #[cfg(not(unix))]
    {
        (0, 0, metadata.len(), 0, 0, 0)
    }
}

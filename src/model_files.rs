use crate::AgentError;
use iquji_agent_wire::PreparedConfigurationFile;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

pub(crate) fn verify_named_files(
    directory: &Path,
    expected: &[(&str, &PreparedConfigurationFile)],
    cancellation: &AtomicBool,
    deadline: Instant,
) -> Result<(), AgentError> {
    if expected.is_empty() {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    let mut names = std::fs::read_dir(directory)
        .map_err(|_| AgentError::ConfigurationMetadataInvalid)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AgentError::ConfigurationMetadataInvalid)?;
    names.sort();
    let mut expected_names = expected
        .iter()
        .map(|(name, _)| std::ffi::OsString::from(name))
        .collect::<Vec<_>>();
    expected_names.sort();
    if names != expected_names {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    for (name, descriptor) in expected {
        verify_file(directory, name, descriptor, cancellation, deadline)?;
    }
    Ok(())
}
fn verify_file(
    directory: &Path,
    name: &str,
    expected: &PreparedConfigurationFile,
    cancellation: &AtomicBool,
    deadline: Instant,
) -> Result<(), AgentError> {
    let path = directory.join(name);
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options
        .open(&path)
        .map_err(|_| AgentError::ConfigurationMetadataInvalid)?;
    let before = file
        .metadata()
        .map_err(|_| AgentError::ConfigurationMetadataInvalid)?;
    if !before.is_file() || before.len() != expected.size || before.len() > 1_048_576 {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.uid() != unsafe { libc::geteuid() }
            || before.mode() & 0o777 != 0o600
            || before.nlink() != 1
        {
            return Err(AgentError::ConfigurationMetadataInvalid);
        }
    }
    let mut digest = Sha256::new();
    let mut total = 0;
    let mut buffer = [0; 8192];
    loop {
        if cancellation.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(AgentError::TurnCancelled);
        }
        if Instant::now() >= deadline {
            return Err(AgentError::TurnTimedOut);
        }
        let count = file
            .read(&mut buffer)
            .map_err(|_| AgentError::ConfigurationMetadataInvalid)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > expected.size {
            return Err(AgentError::ConfigurationMetadataInvalid);
        }
        digest.update(&buffer[..count]);
    }
    if total != expected.size || format!("{:x}", digest.finalize()) != expected.sha256 {
        return Err(AgentError::ConfigurationMetadataInvalid);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let identity = |m: &std::fs::Metadata| {
            (
                m.dev(),
                m.ino(),
                m.len(),
                m.mode(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec(),
            )
        };
        let opened = file
            .metadata()
            .map_err(|_| AgentError::ConfigurationMetadataInvalid)?;
        let after = std::fs::symlink_metadata(&path)
            .map_err(|_| AgentError::ConfigurationMetadataInvalid)?;
        if identity(&before) != identity(&opened) || identity(&before) != identity(&after) {
            return Err(AgentError::ConfigurationMetadataInvalid);
        }
    }
    Ok(())
}

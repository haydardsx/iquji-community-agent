use crate::AgentError;
use std::path::{Path, PathBuf};

pub(crate) struct PrivateDirectory {
    path: PathBuf,
    #[cfg(unix)]
    directory: std::fs::File,
    #[cfg(unix)]
    cleanup_entries: Vec<std::ffi::CString>,
}

impl PrivateDirectory {
    pub(crate) fn new() -> Result<Self, AgentError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let parent =
                std::fs::canonicalize(std::env::temp_dir()).map_err(|_| AgentError::ProcessIo)?;
            for _ in 0..32 {
                let path = parent.join(format!(
                    "iquji-agent-private-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                match std::fs::DirBuilder::new().mode(0o700).create(&path) {
                    Ok(()) => {
                        let directory = std::fs::OpenOptions::new()
                            .read(true)
                            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
                            .open(&path)
                            .map_err(|_| AgentError::ProcessIo)?;
                        let result = Self {
                            path,
                            directory,
                            cleanup_entries: vec![
                                c"model.patch.json".to_owned(),
                                c"settings.json".to_owned(),
                                c"config.toml".to_owned(),
                            ],
                        };
                        result.verify()?;
                        return Ok(result);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(_) => return Err(AgentError::ProcessIo),
                }
            }
            Err(AgentError::ProcessIo)
        }
        #[cfg(not(unix))]
        {
            Err(AgentError::PlatformUnsupported)
        }
    }
    pub(crate) fn verify(&self) -> Result<(), AgentError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let held = self
                .directory
                .metadata()
                .map_err(|_| AgentError::ConfigurationMetadataInvalid)?;
            let current = std::fs::symlink_metadata(&self.path)
                .map_err(|_| AgentError::ConfigurationMetadataInvalid)?;
            if !current.is_dir()
                || current.uid() != unsafe { libc::geteuid() }
                || current.mode() & 0o777 != 0o700
                || current.dev() != held.dev()
                || current.ino() != held.ino()
                || std::fs::canonicalize(&self.path).ok().as_ref() != Some(&self.path)
            {
                return Err(AgentError::ConfigurationMetadataInvalid);
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err(AgentError::PlatformUnsupported)
        }
    }
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(unix)]
    pub(crate) fn register_cleanup_entry(
        &mut self,
        name: &std::ffi::OsStr,
    ) -> Result<(), AgentError> {
        use std::os::unix::ffi::OsStrExt;
        let bytes = name.as_bytes();
        if bytes.is_empty() || bytes.contains(&b'/') {
            return Err(AgentError::ConfigurationMetadataInvalid);
        }
        let name =
            std::ffi::CString::new(bytes).map_err(|_| AgentError::ConfigurationMetadataInvalid)?;
        if !self.cleanup_entries.contains(&name) {
            self.cleanup_entries.push(name);
        }
        Ok(())
    }
}
impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // 路径被替换时只对原目录句柄清理受管文件，不遍历替换后的目标。
            for name in &self.cleanup_entries {
                unsafe { libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0) };
            }
        }
        if self.verify().is_ok() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

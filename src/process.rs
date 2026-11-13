use crate::{AgentError, Configuration};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) struct ManagedChild {
    pub(crate) child: Child,
    owns_group: bool,
    stopped: bool,
}

impl ManagedChild {
    #[cfg(unix)]
    pub(crate) fn spawn(
        program: &Path,
        arguments: &[&str],
        configuration: &Configuration,
        cancelled: &AtomicBool,
        interactive: bool,
    ) -> Result<Self, AgentError> {
        Self::spawn_with_environment(
            program,
            arguments,
            configuration,
            cancelled,
            interactive,
            &std::collections::BTreeMap::new(),
        )
    }

    #[cfg(unix)]
    pub(crate) fn spawn_with_environment(
        program: &Path,
        arguments: &[&str],
        configuration: &Configuration,
        cancelled: &AtomicBool,
        interactive: bool,
        environment: &std::collections::BTreeMap<String, std::ffi::OsString>,
    ) -> Result<Self, AgentError> {
        use std::os::unix::process::CommandExt;
        if cancelled.load(Ordering::SeqCst) {
            return Err(AgentError::ProbeCancelled);
        }
        if std::fs::canonicalize(&configuration.working_directory)
            .ok()
            .as_ref()
            != Some(&configuration.working_directory)
            || !configuration.working_directory.is_dir()
        {
            return Err(AgentError::ConfigurationInvalid);
        }
        let mut command = Command::new(program);
        command
            .args(arguments)
            .env_clear()
            .current_dir(&configuration.working_directory)
            .stdin(if interactive {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(context) = configuration.process_context {
            validate_external_group(context)?;
        } else {
            command.process_group(0);
        }
        if let Some(path) = std::env::var_os("PATH") {
            let directories: Vec<_> = std::env::split_paths(&path)
                .filter(|directory| directory.is_absolute())
                .collect();
            if let Ok(value) = std::env::join_paths(directories) {
                command.env("PATH", value);
            }
        }
        for name in ["LANG", "LC_ALL", "TERM", "TZ"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command.envs(environment);
        Ok(Self {
            child: command.spawn().map_err(|_| AgentError::ProcessFailed)?,
            owns_group: configuration.process_context.is_none(),
            stopped: false,
        })
    }

    pub(crate) fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        #[cfg(unix)]
        if self.owns_group
            && let Ok(group) = i32::try_from(self.child.id())
        {
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(crate) fn validate_external_group(
    context: iquji_agent_wire::ProcessExecutionContext,
) -> Result<(), AgentError> {
    context.validate()?;
    #[cfg(unix)]
    {
        let group = context.owner_process_group as i32;
        if unsafe { libc::getpgrp() } != group
            || unsafe { libc::getpid() } == group
            || unsafe { libc::getpgid(group) } != group
        {
            return Err(AgentError::ConfigurationInvalid);
        }
        Ok(())
    }
    #[cfg(not(unix))]
    Err(AgentError::PlatformUnsupported)
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(unix)]
pub(crate) fn nonblocking(pipe: &impl std::os::fd::AsRawFd) -> Result<(), AgentError> {
    let descriptor = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags == -1
        || unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
    {
        return Err(AgentError::ProcessIo);
    }
    Ok(())
}

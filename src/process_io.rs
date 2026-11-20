use crate::process::{ManagedChild, nonblocking};
use crate::{AgentError, Configuration};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub(crate) struct ProcessBudget {
    pub(crate) deadline: Instant,
    pub(crate) output_left: usize,
}

pub(crate) struct Invocation<'a> {
    pub(crate) program: &'a Path,
    pub(crate) arguments: &'a [&'a str],
    pub(crate) input: Option<&'a [u8]>,
    pub(crate) environment: &'a BTreeMap<String, OsString>,
    pub(crate) output_limit: usize,
}

pub(crate) fn run(
    invocation: Invocation<'_>,
    configuration: &Configuration,
    cancelled: &AtomicBool,
    budget: &mut ProcessBudget,
    on_stdout: impl FnMut(&[u8]) -> Result<(), AgentError>,
) -> Result<(), AgentError> {
    run_observed(
        invocation,
        configuration,
        cancelled,
        budget,
        on_stdout,
        |_| Ok(()),
    )
}

pub(crate) fn run_observed(
    invocation: Invocation<'_>,
    configuration: &Configuration,
    cancelled: &AtomicBool,
    budget: &mut ProcessBudget,
    mut on_stdout: impl FnMut(&[u8]) -> Result<(), AgentError>,
    mut on_stderr: impl FnMut(&[u8]) -> Result<(), AgentError>,
) -> Result<(), AgentError> {
    check(cancelled, budget)?;
    let mut child = ManagedChild::spawn_with_environment(
        invocation.program,
        invocation.arguments,
        configuration,
        cancelled,
        invocation.input.is_some(),
        invocation.environment,
    )?;
    let mut stdout = child.child.stdout.take().ok_or(AgentError::ProcessIo)?;
    let mut stderr = child.child.stderr.take().ok_or(AgentError::ProcessIo)?;
    let mut stdin = child.child.stdin.take();
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    if let Some(pipe) = &stdin {
        nonblocking(pipe)?;
    }
    let input = invocation.input.unwrap_or_default();
    let mut written = 0;
    let mut bytes_read = 0_usize;
    let mut stdout_closed = false;
    let mut stderr_closed = false;
    let mut exit_status = None;
    loop {
        check(cancelled, budget)?;
        let mut progressed = false;
        if written == input.len() {
            stdin.take();
        } else if let Some(pipe) = &mut stdin {
            let end = input.len().min(written + 1_024);
            match pipe.write(&input[written..end]) {
                Ok(0) => return Err(AgentError::ProcessIo),
                Ok(length) => {
                    written += length;
                    progressed = true;
                }
                Err(error) if retry(&error) => {}
                Err(_) => return Err(AgentError::ProcessIo),
            }
        }
        for (reader, closed, capture) in [
            (&mut stdout as &mut dyn Read, &mut stdout_closed, true),
            (&mut stderr as &mut dyn Read, &mut stderr_closed, false),
        ] {
            if *closed {
                continue;
            }
            let mut chunk = [0_u8; 1_024];
            match reader.read(&mut chunk) {
                Ok(0) => *closed = true,
                Ok(length) => {
                    progressed = true;
                    bytes_read = bytes_read.saturating_add(length);
                    if bytes_read > invocation.output_limit || length > budget.output_left {
                        return Err(AgentError::ProbeOutputLimit);
                    }
                    budget.output_left -= length;
                    if capture {
                        on_stdout(&chunk[..length])?;
                    } else {
                        on_stderr(&chunk[..length])?;
                    }
                }
                Err(error) if retry(&error) => {}
                Err(_) => return Err(AgentError::ProcessIo),
            }
        }
        if exit_status.is_none() {
            match child.child.try_wait() {
                Ok(Some(status)) => {
                    exit_status = Some(status);
                    // 入口退出后立即清理同组后代，剩余管道仍受同一期限约束。
                    child.stop();
                }
                Ok(None) => {}
                Err(_) => return Err(AgentError::ProcessIo),
            }
        }
        if let Some(status) = exit_status
            && stdout_closed
            && stderr_closed
        {
            return if status.success() && written == input.len() {
                Ok(())
            } else {
                Err(AgentError::ProcessFailed)
            };
        }
        if !progressed {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

fn check(cancelled: &AtomicBool, budget: &ProcessBudget) -> Result<(), AgentError> {
    if cancelled.load(Ordering::SeqCst) {
        Err(AgentError::ProbeCancelled)
    } else if Instant::now() >= budget.deadline {
        Err(AgentError::ProbeTimedOut)
    } else {
        Ok(())
    }
}

fn retry(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
    )
}

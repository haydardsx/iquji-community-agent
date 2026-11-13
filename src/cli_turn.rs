use iquji_shared_agent::{AgentError, Configuration};
use std::sync::atomic::AtomicBool;

pub(crate) fn configuration_error(error: AgentError) {
    #[cfg(unix)]
    unix::configuration_error(error);
    #[cfg(not(unix))]
    let _ = error;
}

pub(crate) fn diagnostic(error: AgentError) {
    #[cfg(unix)]
    unix::diagnostic(error);
    #[cfg(not(unix))]
    eprintln!("{}", error.code());
}

pub(crate) fn metadata_report(
    value: &impl serde::Serialize,
    configuration: &Configuration,
    cancelled: &AtomicBool,
    started: std::time::Instant,
) -> Result<(), AgentError> {
    #[cfg(unix)]
    {
        unix::metadata_report(value, configuration, cancelled, started)
    }
    #[cfg(not(unix))]
    {
        let _ = (value, configuration, cancelled, started);
        Err(AgentError::PlatformUnsupported)
    }
}

#[cfg(not(unix))]
pub(crate) fn run(
    _configuration: &Configuration,
    _cancelled: &'static AtomicBool,
) -> Result<(), AgentError> {
    Err(AgentError::PlatformUnsupported)
}

#[cfg(unix)]
pub(crate) fn run(
    configuration: &Configuration,
    cancelled: &'static AtomicBool,
) -> Result<(), AgentError> {
    unix::run(configuration, cancelled)
}

#[cfg(unix)]
mod unix {
    use super::*;
    use iquji_agent_wire::{MAX_REQUEST_BYTES, decode_turn_request};
    use iquji_shared_agent::{TurnRequest, TurnTerminal, run_turn};
    use serde::Serialize;
    use std::io;
    use std::os::fd::RawFd;
    use std::sync::{Arc, Mutex, atomic::Ordering};
    use std::time::{Duration, Instant};

    const ERROR_RESERVE: usize = 128;

    struct NonblockingFd {
        descriptor: RawFd,
        flags: i32,
    }
    impl NonblockingFd {
        fn new(descriptor: RawFd) -> Result<Self, AgentError> {
            let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
            if flags == -1
                || unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
            {
                return Err(AgentError::ProcessIo);
            }
            Ok(Self { descriptor, flags })
        }
    }
    impl Drop for NonblockingFd {
        fn drop(&mut self) {
            unsafe {
                libc::fcntl(self.descriptor, libc::F_SETFL, self.flags);
            }
        }
    }

    fn interrupted(cancelled: &AtomicBool, deadline: Instant) -> Result<(), AgentError> {
        if cancelled.load(Ordering::SeqCst) {
            Err(AgentError::TurnCancelled)
        } else if Instant::now() >= deadline {
            Err(AgentError::TurnTimedOut)
        } else {
            Ok(())
        }
    }

    fn wait(
        descriptor: RawFd,
        events: i16,
        cancelled: &AtomicBool,
        deadline: Instant,
    ) -> Result<(), AgentError> {
        interrupted(cancelled, deadline)?;
        let mut descriptor = libc::pollfd {
            fd: descriptor,
            events,
            revents: 0,
        };
        if unsafe { libc::poll(&mut descriptor, 1, 2) } == -1
            && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted
        {
            return Err(AgentError::ProcessIo);
        }
        Ok(())
    }

    fn request(cancelled: &AtomicBool, deadline: Instant) -> Result<TurnRequest, AgentError> {
        let _input = NonblockingFd::new(libc::STDIN_FILENO)?;
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            interrupted(cancelled, deadline)?;
            let size = buffer
                .len()
                .min(MAX_REQUEST_BYTES.saturating_sub(bytes.len()) + 1);
            let length =
                unsafe { libc::read(libc::STDIN_FILENO, buffer.as_mut_ptr().cast(), size) };
            if length == 0 {
                return decode_turn_request(&bytes);
            }
            if length > 0 {
                bytes.extend_from_slice(&buffer[..length as usize]);
                if bytes.len() > MAX_REQUEST_BYTES {
                    return Err(AgentError::RequestInvalid);
                }
            } else {
                match io::Error::last_os_error().kind() {
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => {
                        wait(libc::STDIN_FILENO, libc::POLLIN, cancelled, deadline)?
                    }
                    _ => return Err(AgentError::ProcessIo),
                }
            }
        }
    }

    struct Output {
        _descriptor: NonblockingFd,
        written: usize,
        limit: usize,
        failure: Option<AgentError>,
        complete_line: bool,
    }
    impl Output {
        fn new(limit: usize) -> Result<Self, AgentError> {
            Ok(Self {
                _descriptor: NonblockingFd::new(libc::STDOUT_FILENO)?,
                written: 0,
                limit,
                failure: None,
                complete_line: true,
            })
        }
        fn frame(
            &mut self,
            value: &impl Serialize,
            cancelled: &AtomicBool,
            deadline: Instant,
        ) -> Result<(), AgentError> {
            let mut bytes = serde_json::to_vec(value).map_err(|_| AgentError::ProcessIo)?;
            bytes.push(b'\n');
            if bytes.len()
                > self
                    .limit
                    .saturating_sub(ERROR_RESERVE)
                    .saturating_sub(self.written)
            {
                return Err(AgentError::TurnOutputLimit);
            }
            let mut offset = 0;
            while offset < bytes.len() {
                interrupted(cancelled, deadline)?;
                let length = unsafe {
                    libc::write(
                        libc::STDOUT_FILENO,
                        bytes[offset..].as_ptr().cast(),
                        bytes.len() - offset,
                    )
                };
                if length > 0 {
                    offset += length as usize;
                    self.written += length as usize;
                    self.complete_line = offset == bytes.len();
                } else if length == 0 {
                    return Err(AgentError::ProcessIo);
                } else {
                    match io::Error::last_os_error().kind() {
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => {
                            wait(libc::STDOUT_FILENO, libc::POLLOUT, cancelled, deadline)?
                        }
                        _ => return Err(AgentError::ProcessIo),
                    }
                }
            }
            Ok(())
        }
        fn error(&mut self, error: AgentError) {
            // 已到期限或取消时只能尝试一次有界写；阻塞的消费者不能延长进程寿命。
            if !self.complete_line {
                return;
            }
            let Ok(mut bytes) = serde_json::to_vec(&TurnTerminal::TurnError { error }) else {
                return;
            };
            bytes.push(b'\n');
            if bytes.len() > self.limit.saturating_sub(self.written) {
                return;
            }
            let length =
                unsafe { libc::write(libc::STDOUT_FILENO, bytes.as_ptr().cast(), bytes.len()) };
            if length > 0 {
                self.written += length as usize;
            }
        }
    }

    pub(super) fn metadata_report(
        value: &impl serde::Serialize,
        configuration: &Configuration,
        cancelled: &AtomicBool,
        started: Instant,
    ) -> Result<(), AgentError> {
        let deadline = started + configuration.inspection_timeout();
        let mut output = Output::new(iquji_agent_wire::MAX_TURN_OUTPUT_BYTES)?;
        output
            .frame(value, cancelled, deadline)
            .and_then(|()| interrupted(cancelled, deadline))
            .map_err(|error| match error {
                AgentError::TurnTimedOut => AgentError::ProbeTimedOut,
                AgentError::TurnCancelled => AgentError::ProbeCancelled,
                AgentError::TurnOutputLimit => AgentError::ProbeOutputLimit,
                other => other,
            })
    }

    pub(super) fn configuration_error(error: AgentError) {
        if let Ok(mut output) = Output::new(ERROR_RESERVE) {
            output.error(error);
        }
    }

    pub(super) fn diagnostic(error: AgentError) {
        if let Ok(_descriptor) = NonblockingFd::new(libc::STDERR_FILENO) {
            let bytes = format!("{}\n", error.code());
            unsafe {
                libc::write(libc::STDERR_FILENO, bytes.as_ptr().cast(), bytes.len());
            }
        }
    }

    struct Deadline {
        done: Arc<AtomicBool>,
        expired: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }
    impl Deadline {
        fn new(cancelled: &'static AtomicBool, deadline: Instant) -> Self {
            let done = Arc::new(AtomicBool::new(false));
            let expired = Arc::new(AtomicBool::new(false));
            let completed = done.clone();
            let timed_out = expired.clone();
            let thread = std::thread::spawn(move || {
                while !completed.load(Ordering::SeqCst) && !cancelled.load(Ordering::SeqCst) {
                    if Instant::now() >= deadline {
                        timed_out.store(true, Ordering::SeqCst);
                        cancelled.store(true, Ordering::SeqCst);
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            });
            Self {
                done,
                expired,
                thread: Some(thread),
            }
        }
    }
    impl Drop for Deadline {
        fn drop(&mut self) {
            self.done.store(true, Ordering::SeqCst);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    pub(super) fn run(
        configuration: &Configuration,
        cancelled: &'static AtomicBool,
    ) -> Result<(), AgentError> {
        let deadline = Instant::now() + configuration.turn_timeout();
        let output = Arc::new(Mutex::new(Output::new(configuration.turn_output_limit())?));
        let outcome = (|| {
            let request = request(cancelled, deadline)?;
            let guard = Deadline::new(cancelled, deadline);
            let stream = output.clone();
            let result = run_turn(configuration, request, cancelled, move |event| {
                if let Ok(mut output) = stream.lock()
                    && output.failure.is_none()
                    && let Err(error) = output.frame(&event, cancelled, deadline)
                {
                    output.failure = Some(error);
                    cancelled.store(true, Ordering::SeqCst);
                }
            });
            let mut output = output.lock().map_err(|_| AgentError::ProcessIo)?;
            if guard.expired.load(Ordering::SeqCst)
                && output
                    .failure
                    .is_none_or(|error| error == AgentError::TurnCancelled)
            {
                return Err(AgentError::TurnTimedOut);
            }
            if let Some(error) = output.failure {
                return Err(error);
            }
            let completed = output.frame(
                &TurnTerminal::TurnCompleted { result: result? },
                cancelled,
                deadline,
            );
            match completed {
                Err(AgentError::TurnCancelled) if guard.expired.load(Ordering::SeqCst) => {
                    Err(AgentError::TurnTimedOut)
                }
                outcome => outcome,
            }
        })();
        if let Err(error) = outcome
            && let Ok(mut output) = output.lock()
        {
            output.error(error);
        }
        outcome
    }
}

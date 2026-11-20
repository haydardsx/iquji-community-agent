use crate::process::{ManagedChild, nonblocking};
use crate::{AgentError, Configuration};
use agent_client_protocol::{ByteStreams, TransportBatchEntry, TransportFrame};
use async_io::{Async, Timer};
use futures::{
    AsyncRead,
    future::{Either, select},
};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

struct Budget {
    read: AtomicUsize,
    invalid: AtomicBool,
    limit: usize,
}
impl Budget {
    fn account(&self, count: usize) -> io::Result<()> {
        if self.read.fetch_add(count, Ordering::SeqCst) + count > self.limit {
            Err(io::Error::other("output_limit"))
        } else {
            Ok(())
        }
    }
    fn exceeded(&self) -> bool {
        self.read.load(Ordering::SeqCst) > self.limit
    }
}

pub(crate) struct BoundedRead {
    pipe: Arc<Async<File>>,
    budget: Arc<Budget>,
    pending: Vec<u8>,
}
impl AsyncRead for BoundedRead {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let remaining = this
            .budget
            .limit
            .saturating_sub(this.budget.read.load(Ordering::SeqCst));
        let size = buffer.len().min(remaining + 1);
        let mut pipe = this.pipe.as_ref();
        let result = Pin::new(&mut pipe).poll_read(cx, &mut buffer[..size]);
        match result {
            Poll::Ready(Ok(length)) => {
                if let Err(error) = this.budget.account(length) {
                    return Poll::Ready(Err(error));
                }
                this.pending.extend_from_slice(&buffer[..length]);
                while let Some(end) = this.pending.iter().position(|byte| *byte == b'\n') {
                    let valid =
                        std::str::from_utf8(&this.pending[..end])
                            .ok()
                            .is_some_and(|line| match TransportFrame::parse_json(line) {
                                TransportFrame::Single(_) => true,
                                TransportFrame::Batch(batch) => batch
                                    .entries()
                                    .all(|entry| matches!(entry, TransportBatchEntry::Message(_))),
                                TransportFrame::Malformed { .. } => false,
                            });
                    if !valid {
                        this.budget.invalid.store(true, Ordering::SeqCst);
                        return Poll::Ready(Err(io::Error::other("protocol_invalid")));
                    }
                    this.pending.drain(..=end);
                }
                Poll::Ready(Ok(length))
            }
            result => result,
        }
    }
}

fn drain(descriptor: RawFd, budget: &Budget) -> Result<bool, AgentError> {
    let mut buffer = [0_u8; 1024];
    let read = unsafe { libc::read(descriptor, buffer.as_mut_ptr().cast(), buffer.len()) };
    if read == 0 {
        return Ok(true);
    }
    if read > 0 {
        budget
            .account(read as usize)
            .map_err(|_| AgentError::ProbeOutputLimit)?;
        return Ok(false);
    }
    match io::Error::last_os_error().kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => Ok(false),
        _ => Err(AgentError::ProcessIo),
    }
}

fn interrupted(cancelled: &AtomicBool, deadline: Instant) -> Result<(), AgentError> {
    if cancelled.load(Ordering::SeqCst) {
        Err(AgentError::ProbeCancelled)
    } else if Instant::now() >= deadline {
        Err(AgentError::ProbeTimedOut)
    } else {
        Ok(())
    }
}

pub(crate) type Streams = ByteStreams<Async<std::process::ChildStdin>, BoundedRead>;

pub(crate) fn run<F, Fut>(
    program: &Path,
    configuration: &Configuration,
    cancelled: &AtomicBool,
    timeout: Duration,
    max_output_bytes: usize,
    abort: impl Fn() -> Option<AgentError>,
    connect: F,
) -> Result<(), AgentError>
where
    F: FnOnce(Streams) -> Fut,
    Fut: std::future::Future<Output = Result<(), agent_client_protocol::Error>>,
{
    let mut child = ManagedChild::spawn(program, &[], configuration, cancelled, true)?;
    let deadline = Instant::now() + timeout;
    let stdin = Async::new(child.child.stdin.take().ok_or(AgentError::ProcessIo)?)
        .map_err(|_| AgentError::ProcessIo)?;
    let stdout = Arc::new(
        Async::new(File::from(OwnedFd::from(
            child.child.stdout.take().ok_or(AgentError::ProcessIo)?,
        )))
        .map_err(|_| AgentError::ProcessIo)?,
    );
    let stderr = child.child.stderr.take().ok_or(AgentError::ProcessIo)?;
    nonblocking(&stderr)?;
    let budget = Arc::new(Budget {
        read: AtomicUsize::new(0),
        invalid: AtomicBool::new(false),
        limit: max_output_bytes,
    });
    let incoming = BoundedRead {
        pipe: stdout.clone(),
        budget: budget.clone(),
        pending: Vec::new(),
    };
    // SDK 的协议 trace 可能含原始帧；库嵌入其他宿主时也不能进入其日志订阅。
    let outcome =
        tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), || {
            async_io::block_on(async {
                let negotiation = connect(ByteStreams::new(stdin, incoming));
                let watchdog = async {
                    loop {
                        interrupted(cancelled, deadline)?;
                        if let Some(error) = abort() {
                            return Err(error);
                        }
                        if budget.invalid.load(Ordering::SeqCst) {
                            return Err(AgentError::ProtocolInvalid);
                        }
                        drain(stderr.as_raw_fd(), &budget)?;
                        if let Some(status) =
                            child.child.try_wait().map_err(|_| AgentError::ProcessIo)?
                        {
                            child.stop();
                            if !status.success() {
                                return Err(AgentError::ProcessFailed);
                            }
                        }
                        Timer::after(Duration::from_millis(2)).await;
                    }
                };
                match select(Box::pin(negotiation), Box::pin(watchdog)).await {
                    Either::Left((result, _)) => result.map_err(|_| AgentError::ProtocolInvalid),
                    Either::Right((result, _)) => result,
                }
            })
        });
    child.stop();
    if budget.exceeded() {
        return Err(AgentError::ProbeOutputLimit);
    }
    if budget.invalid.load(Ordering::SeqCst) {
        return Err(AgentError::ProtocolInvalid);
    }
    // 协议任务完成后也回收并排空管道，不能让脱离进程组的后代无限占用读取端。
    let mut closed = [false; 2];
    while !closed.iter().all(|value| *value) {
        interrupted(cancelled, deadline)?;
        for (index, descriptor) in [stdout.get_ref().as_raw_fd(), stderr.as_raw_fd()]
            .into_iter()
            .enumerate()
        {
            if !closed[index] {
                closed[index] = drain(descriptor, &budget)?;
            }
        }
        if !closed.iter().all(|value| *value) {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    if let Some(error) = abort() {
        return Err(error);
    }
    outcome?;
    Ok(())
}

#![cfg(unix)]
mod support;

use iquji_shared_agent::{AgentError, Configuration, scan};
use serde_json::json;
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use support::{CODEX, Fixture, quote};

static PROCESS_TESTS: Mutex<()> = Mutex::new(());

fn process_test() -> MutexGuard<'static, ()> {
    // 计量实际进程期限的回归串行启动，避免并发系统扫描把假 CLI 冷启动当成监管失败。
    PROCESS_TESTS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

struct DetachedPipe {
    root: std::path::PathBuf,
}

impl DetachedPipe {
    fn start(fixture: &Fixture) -> Self {
        fixture.script(&format!(
            "SHARED_AGENT_PIPE_FIXTURE={} exec {} --exact detached_pipe_fixture --nocapture --test-threads=1",
            quote(&fixture.root),
            quote(&std::env::current_exe().unwrap()),
        ));
        Self {
            root: fixture.root.clone(),
        }
    }

    fn started(&self) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.root.join("detached-ready").exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(self.root.join("detached-ready").exists());
    }

    fn entry_exited(&self) {
        let pid: i32 = std::fs::read_to_string(self.root.join("entry-pid"))
            .unwrap()
            .parse()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }
}

impl Drop for DetachedPipe {
    fn drop(&mut self) {
        let _ = std::fs::write(self.root.join("detached-release"), "release");
        let deadline = Instant::now() + Duration::from_secs(1);
        while !self.root.join("detached-done").exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

#[test]
fn detached_pipe_fixture() {
    use std::os::unix::ffi::OsStrExt;
    let Some(root) = std::env::var_os("SHARED_AGENT_PIPE_FIXTURE") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    std::fs::write(root.join("entry-pid"), std::process::id().to_string()).unwrap();
    let path = |name| std::ffi::CString::new(root.join(name).as_os_str().as_bytes()).unwrap();
    let ready = path("detached-ready");
    let release = path("detached-release");
    let done = path("detached-done");
    let mut handshake = [0; 2];
    assert_eq!(unsafe { libc::pipe(handshake.as_mut_ptr()) }, 0);
    let child = unsafe { libc::fork() };
    assert!(child >= 0);
    if child == 0 {
        // fork 后只使用系统调用；脱组后代由测试控制文件主动退出，不按未知 PID 杀进程。
        unsafe {
            libc::close(handshake[0]);
            if libc::setsid() == -1 {
                libc::_exit(90)
            }
            let marker = libc::open(ready.as_ptr(), libc::O_WRONLY | libc::O_CREAT, 0o600);
            if marker == -1 {
                libc::_exit(91)
            }
            libc::close(marker);
            libc::write(handshake[1], b"1".as_ptr().cast(), 1);
            libc::close(handshake[1]);
            for _ in 0..600 {
                if libc::access(release.as_ptr(), libc::F_OK) == 0 {
                    break;
                }
                libc::usleep(5_000);
            }
            let marker = libc::open(done.as_ptr(), libc::O_WRONLY | libc::O_CREAT, 0o600);
            if marker != -1 {
                libc::close(marker);
            }
            libc::_exit(0);
        }
    }
    unsafe {
        libc::close(handshake[1]);
    }
    let mut byte = 0_u8;
    assert_eq!(
        unsafe { libc::read(handshake[0], (&mut byte as *mut u8).cast(), 1) },
        1
    );
    unsafe {
        libc::close(handshake[0]);
    }
    println!("codex-cli 0.50.0");
    std::process::exit(0);
}

#[test]
fn 脱组后代持有管道时正常退出的入口仍受墙钟预算约束() {
    let _serial = process_test();
    let fixture = Fixture::new();
    let detached = DetachedPipe::start(&fixture);
    let mut config = fixture.document();
    config["timeoutMs"] = json!(200);
    fixture.save(config);
    let configuration = Configuration::load(&fixture.config).unwrap();
    let started = Instant::now();
    let report = scan(&configuration, &AtomicBool::new(false));
    let elapsed = started.elapsed();
    detached.started();
    drop(detached);
    assert_eq!(report.connections[0].error, Some(AgentError::ProbeTimedOut));
    assert!(elapsed < Duration::from_secs(1), "{elapsed:?}");
}

#[test]
fn 入口退出后取消无需等待脱组后代释放管道() {
    let _serial = process_test();
    let fixture = Fixture::new();
    let detached = DetachedPipe::start(&fixture);
    let config = Configuration::load(&fixture.config).unwrap();
    let cancellation = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancellation);
    let worker = std::thread::spawn(move || scan(&config, &worker_cancel));
    detached.started();
    detached.entry_exited();
    let started = Instant::now();
    cancellation.store(true, Ordering::SeqCst);
    let report = worker.join().unwrap();
    let elapsed = started.elapsed();
    drop(detached);
    assert_eq!(
        report.connections[0].error,
        Some(AgentError::ProbeCancelled)
    );
    assert!(elapsed < Duration::from_secs(1));
}

#[test]
fn 超时会终止并回收实际子进程且不输出诊断正文() {
    let _serial = process_test();
    let fixture = Fixture::new();
    let pid = fixture.root.join("pid");
    fixture.script(&format!(
        "echo $$ > {}; printf 'secret raw stderr' >&2; /bin/sleep 30",
        quote(&pid)
    ));
    let mut config = fixture.document();
    config["timeoutMs"] = json!(50);
    fixture.save(config);
    let started = Instant::now();
    fixture.assert_error("probe_timed_out");
    assert!(started.elapsed() < Duration::from_secs(3));
    let pid: i32 = std::fs::read_to_string(pid)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
}

#[test]
fn 后代持有输出管道时入口正常退出也不会卡住或遗留活进程() {
    let _serial = process_test();
    let fixture = Fixture::new();
    let escaped = fixture.root.join("escaped");
    fixture.script(&CODEX.replace(
        "printf 'codex-cli 0.50.0\\n'",
        &format!(
            "(/bin/sleep 1; printf 'leaked' > {}) & printf 'codex-cli 0.50.0\\n'",
            quote(&escaped)
        ),
    ));
    let started = Instant::now();
    let report = fixture.report();
    assert_eq!(report["connections"][0]["status"], "detected", "{report}");
    assert!(started.elapsed() < Duration::from_secs(3));
    std::thread::sleep(Duration::from_millis(1_100));
    assert!(!escaped.exists());
}

#[test]
fn 标准输出与错误输出共享硬上限并在超限后回收进程() {
    let _serial = process_test();
    let fixture = Fixture::new();
    let mut config = fixture.document();
    config["maxOutputBytes"] = json!(128);
    fixture.save(config);
    for stream in ["", ">&2"] {
        fixture.script(&format!(
            "while :; do printf '%s' 'unbounded-private-output-0123456789' {stream}; done"
        ));
        let started = Instant::now();
        fixture.assert_error("probe_output_limit");
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}

#[test]
fn 原生取消在清理完成后返回稳定结果() {
    let _serial = process_test();
    let fixture = Fixture::new();
    let pid = fixture.root.join("cancel-pid");
    fixture.script(&format!("echo $$ > {}; /bin/sleep 30", quote(&pid)));
    let config = Configuration::load(&fixture.config).unwrap();
    let cancellation = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancellation);
    let worker = std::thread::spawn(move || scan(&config, &worker_cancel));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !pid.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(pid.exists());
    cancellation.store(true, Ordering::SeqCst);
    let report = worker.join().unwrap();
    assert_eq!(
        report.connections[0].error,
        Some(AgentError::ProbeCancelled)
    );
    let pid: i32 = std::fs::read_to_string(pid)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
}

#[test]
fn 外部终止信号转换为取消并回收独立进程组() {
    let _serial = process_test();
    let fixture = Fixture::new();
    let pid = fixture.root.join("signal-pid");
    fixture.script(&format!("echo $$ > {}; /bin/sleep 30", quote(&pid)));
    let mut command = fixture.command();
    let child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !pid.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(pid.exists());
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"probe_cancelled\n");
    let pid: i32 = std::fs::read_to_string(pid)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
}

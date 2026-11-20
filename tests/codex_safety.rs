#![cfg(unix)]
mod support;
use iquji_shared_agent::AgentError;
use serde_json::json;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use support::codex::*;
#[test]
fn codex_peer_fixture() {
    peer();
}
#[test]
fn codex_detached_fixture() {
    detached_peer();
}

#[test]
fn 超时取消与合计输出超限均回收入口和自有配置目录() {
    for (scenario, error) in [
        ("hang", AgentError::TurnTimedOut),
        ("flood", AgentError::TurnOutputLimit),
        ("no-read", AgentError::TurnTimedOut),
    ] {
        let fixture = fixture(scenario, 400, 4096);
        let start = Instant::now();
        let (result, _) = invoke(&fixture, None, &AtomicBool::new(false));
        assert_eq!(result.unwrap_err(), error, "{scenario}");
        assert!(start.elapsed() < Duration::from_secs(5));
        assert_cleaned(&fixture);
        assert_reaped(&fixture);
    }
    let fixture = fixture("hang", 3000, 4096);
    let cancelled = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            wait_file(&fixture.root.join("record.prompt"));
            cancelled.store(true, Ordering::SeqCst);
        });
        assert_eq!(
            invoke(&fixture, None, &cancelled).0.unwrap_err(),
            AgentError::TurnCancelled
        );
    });
    assert_cleaned(&fixture);
    assert_reaped(&fixture);
}

#[test]
fn 脱组后代持有管道不能拖住完成且工具不猜测杀进程() {
    let fixture = fixture("detach", 2400, 32768);
    let start = Instant::now();
    let result = invoke(&fixture, None, &AtomicBool::new(false)).0;
    let pid: i32 = std::fs::read_to_string(fixture.root.join("record.detached"))
        .unwrap()
        .parse()
        .unwrap();
    // 此PID由本测试直接创建并记录；工具只关闭自己的管道，不终止脱组PID。
    let alive = unsafe { libc::kill(pid, 0) };
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    assert_eq!(alive, 0);
    assert_eq!(result.unwrap_err(), AgentError::TurnTimedOut);
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_cleaned(&fixture);
    assert_reaped(&fixture);
}

#[test]
fn 元数据失败也清理隔离目录且不触碰显式用户目录() {
    let fixture = fixture("normal", 3000, 32768);
    let user = fixture.root.join("user-config");
    std::fs::create_dir(&user).unwrap();
    std::fs::write(user.join("untouched"), "preserved").unwrap();
    let mut config = fixture.document();
    config["agents"][0]["codexConfigurationDirectory"] = json!(user);
    fixture.save(config);
    fixture.script(&format!("printf '%s\\t%s\\tunset\\tunset\\n' \"$HOME\" \"$CODEX_HOME\" >> {}\nprintf 'codex-cli 0.1.0\\n'",support::quote(&fixture.root.join("metadata"))));
    assert_eq!(
        invoke(&fixture, None, &AtomicBool::new(false))
            .0
            .unwrap_err(),
        AgentError::VersionUnsupported
    );
    assert_eq!(
        std::fs::read_to_string(user.join("untouched")).unwrap(),
        "preserved"
    );
    assert_cleaned(&fixture);
}

#[test]
fn 命令行信号取消清理且完成输出背压仍是超时() {
    let fixture = fixture("hang", 3000, 32768);
    let mut child = command(&fixture).spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(r#"{"agentId":"fixture","prompt":"问题"}"#.as_bytes())
        .unwrap();
    wait_file(&fixture.root.join("record.prompt"));
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let output = bounded(child);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stderr, b"turn_cancelled\n");
    assert_cleaned(&fixture);
    assert_reaped(&fixture);
    let fixture = support::codex::fixture("large", 1800, 524288);
    let mut child = command(&fixture).spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(r#"{"agentId":"fixture","prompt":"问题"}"#.as_bytes())
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    // 缩小管道仅为Linux加速；其他Unix仍由大于管道容量的合法文本制造背压。
    #[cfg(target_os = "linux")]
    unsafe {
        libc::fcntl(stdout.as_raw_fd(), libc::F_SETPIPE_SZ, 4096);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = stdout.as_raw_fd();
    let start = Instant::now();
    let output = bounded(child);
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_eq!(output.stderr, b"turn_timed_out\n");
    let mut ignored = Vec::new();
    std::io::Read::read_to_end(&mut stdout, &mut ignored).unwrap();
    assert_cleaned(&fixture);
    assert_reaped(&fixture);
}

#[test]
fn 显式用户目录在错误超时与输出超限后仍原样保留() {
    for scenario in ["failed", "hang", "flood"] {
        let fixture = support::codex::fixture(scenario, 400, 4096);
        let user = fixture.root.join("user-config");
        std::fs::create_dir(&user).unwrap();
        support::private_mode(&user, 0o750);
        std::fs::write(user.join("provider-config"), "preserved").unwrap();
        let mut config = fixture.document();
        config["turnTimeoutMs"] = json!(400);
        config["maxTurnOutputBytes"] = json!(4096);
        config["agents"][0]["codexConfigurationDirectory"] = json!(user);
        fixture.save(config);
        assert!(invoke(&fixture, None, &AtomicBool::new(false)).0.is_err());
        assert_eq!(
            std::fs::read_to_string(user.join("provider-config")).unwrap(),
            "preserved"
        );
        assert_eq!(record(&fixture)["codexMode"], 0o750);
        assert_cleaned(&fixture);
        assert_reaped(&fixture);
    }
}

#[test]
fn 元数据与执行共享总期限与字节预算() {
    let fixture = support::codex::fixture("normal", 250, 32768);
    let script = std::fs::read_to_string(&fixture.program).unwrap();
    fixture.script(&script.replace("case \"$*\" in", "sleep 0.1\ncase \"$*\" in"));
    let start = Instant::now();
    assert_eq!(
        invoke(&fixture, None, &AtomicBool::new(false))
            .0
            .unwrap_err(),
        AgentError::TurnTimedOut
    );
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(!fixture.root.join("record").exists());
    assert_cleaned(&fixture);
    let fixture = support::codex::fixture("normal", 3000, 256);
    let script = std::fs::read_to_string(&fixture.program).unwrap();
    fixture.script(&script.replace(
        "printf 'codex-cli",
        "printf '%0120d' 0 >&2\nprintf 'codex-cli",
    ));
    assert_eq!(
        invoke(&fixture, None, &AtomicBool::new(false))
            .0
            .unwrap_err(),
        AgentError::TurnOutputLimit
    );
    assert_cleaned(&fixture);
}

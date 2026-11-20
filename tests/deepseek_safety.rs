#![cfg(unix)]
mod support;
use iquji_shared_agent::AgentError;
use serde_json::json;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use support::{
    codex::{bounded, command, invoke, wait_file},
    deepseek::*,
};
#[test]
fn deepseek_peer_fixture() {
    peer();
}
#[test]
fn deepseek_detached_fixture() {
    detached_peer();
}

#[test]
fn 超时取消与合计输出预算使用同一监管执行器() {
    for (scenario, error) in [
        ("hang", AgentError::TurnTimedOut),
        ("flood", AgentError::TurnOutputLimit),
    ] {
        let fixture = fixture(scenario, 1200, 4096);
        let start = Instant::now();
        assert_eq!(
            invoke(&fixture, None, &AtomicBool::new(false))
                .0
                .unwrap_err(),
            error
        );
        assert!(start.elapsed() < Duration::from_secs(5));
        assert_cleaned(&fixture);
        assert_reaped(&fixture);
    }
    let fixture = fixture("hang", 3000, 32768);
    let cancelled = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            wait_file(&fixture.root.join("record"));
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
fn 脱组持管道不能伪造成功也不被工具猜测终止() {
    let fixture = fixture("detach", 2400, 32768);
    let start = Instant::now();
    let result = invoke(&fixture, None, &AtomicBool::new(false)).0;
    let pid: i32 = std::fs::read_to_string(fixture.root.join("record.detached"))
        .unwrap()
        .parse()
        .unwrap();
    let alive = unsafe { libc::kill(pid, 0) };
    // PID由本测试创建并记录，只有外层回归负责终止脱组后代。
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
fn 信号与最终stdout背压保持稳定错误和临时目录清理() {
    let fixture = fixture("hang", 3000, 32768);
    let mut child = command(&fixture).spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&json!({"agentId":"fixture","prompt":"问题"})).unwrap())
        .unwrap();
    wait_file(&fixture.root.join("record"));
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let output = bounded(child);
    assert_eq!(output.stderr, b"turn_cancelled\n");
    assert_cleaned(&fixture);
    assert_reaped(&fixture);
    let fixture = support::deepseek::fixture("large", 1800, 524288);
    let mut child = command(&fixture).spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&json!({"agentId":"fixture","prompt":"问题"})).unwrap())
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let output = bounded(child);
    assert_eq!(output.stderr, b"turn_timed_out\n");
    let mut ignored = Vec::new();
    stdout.read_to_end(&mut ignored).unwrap();
    assert_cleaned(&fixture);
    assert_reaped(&fixture);
}

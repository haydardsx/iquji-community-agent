#![cfg(unix)]
mod support;

use iquji_agent_wire::PROCESS_GROUP_OWNER_ARGUMENT;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use support::{CODEX, Fixture, quote};

#[test]
fn external_owner_fixture() {
    if std::env::var_os("SHARED_PROCESS_CONTEXT_FIXTURE").is_none() {
        return;
    }
    let owner = std::process::id();
    assert_eq!(unsafe { libc::getpgrp() }, owner as i32);
    let fixture = Fixture::new();
    let groups = fixture.root.join("groups");
    fixture.script(&format!(
        "/bin/ps -o pgid= -p $$ >> {}\n{CODEX}",
        quote(&groups)
    ));
    let mut sibling = Command::new("/bin/sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let output = fixture
        .command()
        .arg(PROCESS_GROUP_OWNER_ARGUMENT)
        .arg(owner.to_string())
        .output()
        .unwrap();
    let sibling_alive = sibling.try_wait().unwrap().is_none();
    let _ = sibling.kill();
    let _ = sibling.wait();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(sibling_alive, "工具清理不得终止外部owner组内的其他入口");
    let observed = std::fs::read_to_string(groups).unwrap();
    assert!(!observed.trim().is_empty());
    assert!(
        observed
            .lines()
            .all(|line| line.trim().parse::<u32>().unwrap() == owner)
    );
}

#[test]
fn 显式外部归属使工具及命令行继承同一进程组且不误杀组内其他入口() {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "external_owner_fixture", "--nocapture"])
        .env("SHARED_PROCESS_CONTEXT_FIXTURE", "1")
        .process_group(0)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn 外部组长与当前进程组不符时在启动命令行前拒绝() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .arg(PROCESS_GROUP_OWNER_ARGUMENT)
        .arg(std::process::id().to_string())
        .process_group(0)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stderr, b"configuration_invalid\n");
    assert!(!fixture.calls.exists());
}

#[test]
fn 持久配置不能选择外部进程归属() {
    let fixture = Fixture::new();
    let mut document = fixture.document();
    document["processContext"] = serde_json::json!({"ownerProcessGroup": std::process::id()});
    fixture.save(document);
    let output = fixture.command().output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stderr, b"configuration_invalid\n");
    assert!(!fixture.calls.exists());
}

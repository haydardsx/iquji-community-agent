use iquji_agent_wire::{AgentError, TurnOutput, TurnTerminal, decode_turn_output};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn independent_process_host_uses_only_the_public_wire() {
    let package = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let directory = Temporary(std::env::temp_dir().join(format!(
            "agent-wire-host-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
    std::fs::create_dir(&directory.0).unwrap();
    std::fs::create_dir(directory.0.join("src")).unwrap();
    // 唯一源码依赖显式指向已检出的公开叶子；没有 Agent 实现或产品合同。
    let manifest = include_str!("fixtures/host/Cargo.toml").replace(
        "path = \"../../..\"",
        &format!(
            "path = {}",
            serde_json::to_string(&package.to_string_lossy()).unwrap()
        ),
    );
    std::fs::write(directory.0.join("Cargo.toml"), manifest).unwrap();
    std::fs::write(
        directory.0.join("Cargo.lock"),
        include_bytes!("fixtures/host/Cargo.lock"),
    )
    .unwrap();
    std::fs::write(
        directory.0.join("src/main.rs"),
        include_bytes!("fixtures/host/src/main.rs"),
    )
    .unwrap();
    let build = Command::new(env!("CARGO"))
        .args(["build", "--offline", "--locked", "--quiet"])
        .current_dir(&directory.0)
        .env("CARGO_TARGET_DIR", directory.0.join("target"))
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let program = directory.0.join("target/debug/agent-wire-fixture-host");
    for (request, expected) in [
        (br#"{"agentId":"fake","prompt":"hello"}"#.as_slice(), None),
        (
            br#"{"agentId":"fake","prompt":"hello","sessionId":"real-but-unsupported"}"#.as_slice(),
            Some(AgentError::SessionUnsupported),
        ),
        (
            br#"{"agentId":"fake","prompt":"hello","environment":{"SECRET":"value"}}"#.as_slice(),
            Some(AgentError::RequestInvalid),
        ),
    ] {
        let mut child = Command::new(&program)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(request).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.stderr.is_empty());
        let terminal = decode_turn_output(&output.stdout).unwrap();
        match (expected, terminal) {
            (None, TurnOutput::Terminal(TurnTerminal::TurnCompleted { result })) => {
                assert!(output.status.success());
                assert_eq!(result.text, "hello");
                assert_eq!(result.session_id, None);
            }
            (Some(expected), TurnOutput::Terminal(TurnTerminal::TurnError { error })) => {
                assert_eq!(output.status.code(), Some(2));
                assert_eq!(error, expected);
            }
            _ => panic!("unexpected wire terminal"),
        }
    }
}

#![cfg(unix)]
mod support;

use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};
use support::{Fixture, quote};

fn turn_fixture(scenario: &str, timeout: u64, limit: usize) -> Fixture {
    static READY: std::sync::Once = std::sync::Once::new();
    READY.call_once(|| {
        // 单独预热新构建的CLI入口，墙钟回归不混入系统首次可执行扫描。
        assert_eq!(
            Command::new(env!("CARGO_BIN_EXE_iquji-shared-agent"))
                .env_clear()
                .output()
                .unwrap()
                .status
                .code(),
            Some(2)
        );
    });
    let fixture = Fixture::new();
    fixture.script(&format!(
        "SHARED_AGENT_CLI_TURN_SCENARIO='{scenario}' SHARED_AGENT_CLI_TURN_RECORD={} exec {} --exact cli_turn_peer --nocapture 3>&1 1>/dev/null",
        quote(&fixture.root.join("turn.jsonl")), quote(&std::env::current_exe().unwrap()),
    ));
    let mut config = fixture.document();
    config["agents"][0]["adapter"] = json!("native_acp");
    config["turnTimeoutMs"] = json!(timeout);
    config["maxTurnOutputBytes"] = json!(limit);
    fixture.save(config);
    fixture
}
fn command(fixture: &Fixture) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_iquji-shared-agent"));
    command
        .args([
            std::ffi::OsStr::new("--config"),
            fixture.config.as_os_str(),
            std::ffi::OsStr::new("turn"),
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(&fixture.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}
fn request(session: Option<&str>) -> Value {
    json!({"agentId":"fixture", "prompt":"本次问题", "sessionId":session})
}
fn run(fixture: &Fixture, bytes: &[u8]) -> Output {
    let mut child = command(fixture).spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    let _ = input.write_all(bytes);
    drop(input);
    bounded_output(child)
}
fn bounded_output(mut child: Child) -> Output {
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("单次CLI未在期限内退出");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    child.wait_with_output().unwrap()
}
fn frames(output: &Output) -> Vec<Value> {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn assert_error(output: &Output, error: &str) {
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!("{error}\n")
    );
    assert_eq!(
        frames(output).last().unwrap(),
        &json!({"kind":"turn_error","error":error})
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("sensitive"));
}
fn wait_file(file: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !file.exists() {
        assert!(Instant::now() < deadline, "假ACP没有进入回合");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn assert_reaped(fixture: &Fixture) {
    let pid: i32 = std::fs::read_to_string(fixture.root.join("turn.pid"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

#[test]
fn cli_turn_peer() {
    let Ok(scenario) = std::env::var("SHARED_AGENT_CLI_TURN_SCENARIO") else {
        return;
    };
    let record =
        std::path::PathBuf::from(std::env::var_os("SHARED_AGENT_CLI_TURN_RECORD").unwrap());
    std::fs::write(record.with_extension("pid"), std::process::id().to_string()).unwrap();
    let mut calls = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&record)
        .unwrap();
    let mut pipe = unsafe { std::fs::File::from_raw_fd(3) };
    let mut input = std::io::stdin().lock();
    let mut read = || {
        let mut line = String::new();
        input.read_line(&mut line).unwrap();
        calls.write_all(line.as_bytes()).unwrap();
        calls.flush().unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    };
    let initialize = read();
    assert_eq!(initialize["method"], "initialize");
    assert_eq!(std::env::args_os().count(), 4);
    for name in ["HOME", "OPENAI_API_KEY", "HTTP_PROXY", "CODEX_HOME"] {
        assert!(std::env::var_os(name).is_none());
    }
    frame(
        &mut pipe,
        json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":scenario!="connection"},"authMethods":[]}}),
    );
    let setup = read();
    assert_eq!(
        setup["params"]["cwd"],
        std::env::current_dir().unwrap().to_str().unwrap()
    );
    if scenario == "auth" {
        frame(
            &mut pipe,
            json!({"jsonrpc":"2.0","id":setup["id"],"error":{"code":-32000,"message":"sensitive private provider", "data":{"token":"sensitive"}}}),
        );
    } else {
        assert!(setup["params"]["mcpServers"].as_array().unwrap().is_empty());
        if setup["method"] == "session/new" {
            frame(
                &mut pipe,
                json!({"jsonrpc":"2.0","id":setup["id"],"result":{"sessionId":"opaque-cli-session"}}),
            );
        } else {
            assert_eq!(setup["method"], "session/load");
            assert_eq!(setup["params"]["sessionId"], "opaque-cli-session");
            update(&mut pipe, "历史不能重放");
            frame(
                &mut pipe,
                json!({"jsonrpc":"2.0","id":setup["id"],"result":{}}),
            );
        }
        let prompt = read();
        assert_eq!(prompt["method"], "session/prompt");
        std::fs::write(record.with_extension("prompt"), "ready").unwrap();
        match scenario.as_str() {
            "hang" => {}
            "flood" => loop {
                update(&mut pipe, &"x".repeat(1000));
                // 让消费者真实进入stdout背压，而不是先耗尽ACP原始帧预算。
                std::thread::sleep(Duration::from_millis(2));
            },
            "medium" => {
                update(&mut pipe, &"x".repeat(1100));
                frame(
                    &mut pipe,
                    json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"end_turn"}}),
                );
            }
            "terminal" => {
                update(&mut pipe, &"x".repeat(200_000));
                frame(
                    &mut pipe,
                    json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"end_turn"}}),
                );
            }
            _ => {
                update(&mut pipe, "实际答案\n");
                frame(
                    &mut pipe,
                    json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"end_turn"}}),
                );
            }
        }
    }
    std::thread::sleep(Duration::from_secs(60));
}
fn frame(pipe: &mut std::fs::File, value: Value) {
    writeln!(pipe, "{value}").unwrap();
    pipe.flush().unwrap();
}
fn update(pipe: &mut std::fs::File, text: &str) {
    frame(
        pipe,
        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"opaque-cli-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}}}),
    );
}

#[test]
fn cli_projects_real_turn_events_and_final_without_private_environment_or_extra_argv() {
    let fixture = turn_fixture("normal", 3000, 16384);
    let mut process = command(&fixture);
    process
        .env("HOME", "/sensitive/home")
        .env("OPENAI_API_KEY", "sensitive")
        .env("HTTP_PROXY", "sensitive")
        .env("CODEX_HOME", "sensitive");
    let mut child = process.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request(None).to_string().as_bytes())
        .unwrap();
    let output = bounded_output(child);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(std::fs::read_to_string(&fixture.calls).unwrap(), "\n");
    assert_eq!(
        frames(&output),
        vec![
            json!({"kind":"session_started","sessionId":"opaque-cli-session","resumable":true}),
            json!({"kind":"text_delta","text":"实际答案\n"}),
            json!({"kind":"turn_completed","sessionId":"opaque-cli-session","resumable":true,"text":"实际答案\n","stopReason":"end_turn"})
        ]
    );
    assert_reaped(&fixture);
}

#[test]
fn each_cli_invocation_initializes_a_fresh_connection_and_only_load_session_allows_resume() {
    let fixture = turn_fixture("normal", 3000, 16384);
    for session in [None, Some("opaque-cli-session")] {
        let output = run(&fixture, request(session).to_string().as_bytes());
        assert!(output.status.success());
        assert_eq!(frames(&output).last().unwrap()["text"], "实际答案\n");
        assert_reaped(&fixture);
    }
    let calls = std::fs::read_to_string(fixture.root.join("turn.jsonl")).unwrap();
    let methods: Vec<_> = calls
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["method"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "session/new",
            "session/prompt",
            "initialize",
            "session/load",
            "session/prompt"
        ]
    );
    let fixture = turn_fixture("connection", 3000, 16384);
    assert_error(
        &run(
            &fixture,
            request(Some("opaque-cli-session")).to_string().as_bytes(),
        ),
        "session_unsupported",
    );
    assert_reaped(&fixture);
}

#[test]
fn stdin_requires_one_bounded_request_and_rejects_injection_before_starting_agent() {
    let fixture = turn_fixture("normal", 3000, 16384);
    for input in [
        b"".to_vec(),
        b"{}".to_vec(),
        vec![0xff],
        [request(None).to_string(), request(None).to_string()]
            .join("\n")
            .into_bytes(),
        json!({"agentId":"fixture","prompt":"valid","command":"sensitive"})
            .to_string()
            .into_bytes(),
        json!({"agentId":"fixture","prompt":"x".repeat(65_537)})
            .to_string()
            .into_bytes(),
        vec![b' '; 400 * 1024 + 1],
    ] {
        assert_error(&run(&fixture, &input), "request_invalid");
    }
    assert!(!fixture.calls.exists());
}

#[test]
fn bounded_encoding_still_accepts_a_valid_maximum_length_escaped_prompt() {
    let fixture = turn_fixture("normal", 3000, 16384);
    let input = format!(
        r#"{{"agentId":"fixture","prompt":"x{}"}}"#,
        r"\u000a".repeat(65_535)
    );
    let output = run(&fixture, input.as_bytes());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(frames(&output).last().unwrap()["kind"], "turn_completed");
    assert_reaped(&fixture);
}

#[test]
fn turn_does_not_infer_configuration_or_accept_an_unknown_agent() {
    let fixture = turn_fixture("normal", 3000, 16384);
    let mut unknown = request(None);
    unknown["agentId"] = json!("missing");
    assert_error(
        &run(&fixture, unknown.to_string().as_bytes()),
        "request_invalid",
    );
    assert!(!fixture.calls.exists());
    let output = Command::new(env!("CARGO_BIN_EXE_iquji-shared-agent"))
        .arg("turn")
        .env_clear()
        .current_dir(&fixture.root)
        .output()
        .unwrap();
    fixture.assert_configuration_error(output, "configuration_invalid");
}

#[test]
fn stdin_without_eof_has_the_same_wall_clock_budget_and_signal_cancellation() {
    let fixture = turn_fixture("normal", 150, 16384);
    let mut child = command(&fixture).spawn().unwrap();
    let _input = child.stdin.take().unwrap();
    assert_error(&bounded_output(child), "turn_timed_out");
    assert!(!fixture.calls.exists());
    use std::os::unix::process::CommandExt;
    let mut process = command(&fixture);
    // 启动扫描期间先忽略信号；反复发送直到正式main安装处理器，避免测到exec前默认终止。
    unsafe {
        process.pre_exec(|| {
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
            Ok(())
        });
    }
    let mut child = process.spawn().unwrap();
    let _input = child.stdin.take().unwrap();
    for _ in 0..600 {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_error(&bounded_output(child), "turn_cancelled");
}

#[test]
fn stdout_has_its_own_total_byte_budget_including_repeated_final_text() {
    let fixture = turn_fixture("medium", 3000, 2048);
    let output = run(&fixture, request(None).to_string().as_bytes());
    assert_error(&output, "turn_output_limit");
    assert!(output.stdout.len() <= 2048);
    assert!(
        frames(&output)
            .iter()
            .any(|event| event["kind"] == "text_delta")
    );
    assert_reaped(&fixture);
}

#[test]
fn stdout_backpressure_does_not_hold_the_peer_or_cli_past_the_deadline() {
    for signal in [None, Some(libc::SIGTERM)] {
        let fixture = turn_fixture(
            "flood",
            if signal.is_some() { 3000 } else { 1200 },
            4 * 1024 * 1024,
        );
        let (reader, writer) = std::os::unix::net::UnixStream::pair().unwrap();
        let size: libc::c_int = 4096;
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    writer.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&size as *const libc::c_int).cast(),
                    std::mem::size_of_val(&size) as libc::socklen_t,
                )
            },
            0
        );
        // 固定小发送缓冲区，避免操作系统扩张pipe后先触发字节上限，实际停读端保持存活。
        let mut child = command(&fixture)
            .stdout(Stdio::from(OwnedFd::from(writer)))
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request(None).to_string().as_bytes())
            .unwrap();
        if let Some(signal) = signal {
            wait_file(&fixture.root.join("turn.prompt"));
            std::thread::sleep(Duration::from_millis(40));
            unsafe {
                libc::kill(child.id() as i32, signal);
            }
        }
        let output = bounded_output(child);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            if signal.is_some() {
                "turn_cancelled\n"
            } else {
                "turn_timed_out\n"
            }
        );
        assert_reaped(&fixture);
        drop(reader);
    }
}

#[test]
fn closed_stdout_is_process_io_instead_of_cancellation_and_still_reaps_the_peer() {
    let fixture = turn_fixture("normal", 3000, 16384);
    let (reader, writer) = std::os::unix::net::UnixStream::pair().unwrap();
    drop(reader);
    let mut child = command(&fixture)
        .stdout(Stdio::from(OwnedFd::from(writer)))
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request(None).to_string().as_bytes())
        .unwrap();
    let output = bounded_output(child);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&output.stderr), "process_io\n");
    assert_reaped(&fixture);
}

#[test]
fn sigint_and_sigterm_cancel_the_real_active_turn_and_reap_its_entry_process() {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let fixture = turn_fixture("hang", 3000, 16384);
        let mut child = command(&fixture).spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request(None).to_string().as_bytes())
            .unwrap();
        wait_file(&fixture.root.join("turn.prompt"));
        unsafe {
            libc::kill(child.id() as i32, signal);
        }
        assert_error(&bounded_output(child), "turn_cancelled");
        assert_reaped(&fixture);
    }
}

#[test]
fn explicit_turn_config_and_official_authentication_errors_keep_stable_diagnostics() {
    let fixture = turn_fixture("auth", 3000, 16384);
    assert_error(
        &run(&fixture, request(None).to_string().as_bytes()),
        "authentication_required",
    );
    assert_reaped(&fixture);
    std::fs::remove_file(&fixture.config).unwrap();
    assert_error(&run(&fixture, b"{}"), "configuration_invalid");
}

#[test]
fn completed_frame_backpressure_keeps_deadline_and_native_cancellation_distinct() {
    for signal in [None, Some(libc::SIGTERM)] {
        let fixture = turn_fixture(
            "terminal",
            if signal.is_some() { 3000 } else { 1500 },
            1024 * 1024,
        );
        let (reader, writer) = std::os::unix::net::UnixStream::pair().unwrap();
        reader
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let size: libc::c_int = 4096;
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    writer.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&size as *const libc::c_int).cast(),
                    std::mem::size_of_val(&size) as libc::socklen_t,
                )
            },
            0
        );
        let mut child = command(&fixture)
            .stdout(Stdio::from(OwnedFd::from(writer)))
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request(None).to_string().as_bytes())
            .unwrap();
        let mut reader = std::io::BufReader::new(reader);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap()["kind"],
            "session_started"
        );
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap()["kind"],
            "text_delta"
        );
        // 已完整消费delta后停读，使重复累积文本的最终帧单独承受背压。
        if let Some(signal) = signal {
            unsafe {
                libc::kill(child.id() as i32, signal);
            }
        }
        let output = bounded_output(child);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            if signal.is_some() {
                "turn_cancelled\n"
            } else {
                "turn_timed_out\n"
            }
        );
        assert_reaped(&fixture);
        drop(reader);
    }
}

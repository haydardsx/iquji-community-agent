#![cfg(unix)]
mod support;

use iquji_shared_agent::{AgentError, Configuration, TurnRequest, run_turn};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::os::fd::FromRawFd;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use support::{Fixture, quote};

fn fixture(scenario: &str) -> Fixture {
    let fixture = Fixture::new();
    fixture.script(&format!(
        "SHARED_AGENT_TURN_FIXTURE='{scenario}' SHARED_AGENT_TURN_RECORD={} exec {} --exact turn_peer_fixture --nocapture 3>&1 1>/dev/null",
        quote(&fixture.root.join("turn.jsonl")), quote(&std::env::current_exe().unwrap()),
    ));
    let mut config = fixture.document();
    config["agents"][0]["adapter"] = json!("native_acp");
    config["turnTimeoutMs"] = json!(1500);
    fixture.save(config);
    fixture
}

fn request(session: Option<&str>) -> TurnRequest {
    TurnRequest {
        workspace_write: false,
        agent_id: "fixture".into(),
        context_fingerprint: None,
        prompt: "本次问题".into(),
        session_id: session.map(str::to_owned),
        model: None,
        attachments: Vec::new(),
        mcp_servers: Vec::new(),
    }
}

fn request_with_mcp(session: Option<&str>) -> TurnRequest {
    let mut request = request(session);
    request.mcp_servers.push(iquji_agent_wire::McpServer {
        name: "project_tools".into(),
        url: "http://127.0.0.1:43127/mcp/0123456789abcdef0123456789abcdef0123456789abcdef".into(),
    });
    request
}

#[test]
fn turn_peer_fixture() {
    let Ok(scenario) = std::env::var("SHARED_AGENT_TURN_FIXTURE") else {
        return;
    };
    let record = std::path::PathBuf::from(std::env::var_os("SHARED_AGENT_TURN_RECORD").unwrap());
    std::fs::write(record.with_extension("pid"), std::process::id().to_string()).unwrap();
    let mut calls = std::fs::File::create(&record).unwrap();
    let mut pipe = unsafe { std::fs::File::from_raw_fd(3) };
    let mut input = std::io::stdin().lock();
    let mut read = || {
        let mut line = String::new();
        input.read_line(&mut line).unwrap();
        calls.write_all(line.as_bytes()).unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    };
    let initialized = read();
    assert_eq!(initialized["method"], "initialize");
    write_frame(
        &mut pipe,
        json!({"jsonrpc":"2.0", "id":initialized["id"], "result":{
            "protocolVersion":1,
            "agentCapabilities":{
                "loadSession":!matches!(scenario.as_str(), "no-load" | "connection"),
                "mcpCapabilities":{"http":matches!(scenario.as_str(), "mcp-new" | "mcp-load")}
            },
            "authMethods":[]
        }}),
    );
    if matches!(scenario.as_str(), "no-load" | "mcp-unsupported") {
        std::thread::sleep(Duration::from_secs(60));
        return;
    }
    let setup = read();
    if matches!(scenario.as_str(), "mcp-new" | "mcp-load") {
        assert_eq!(
            setup["params"]["mcpServers"],
            json!([{
                "type":"http",
                "name":"project_tools",
                "url":"http://127.0.0.1:43127/mcp/0123456789abcdef0123456789abcdef0123456789abcdef",
                "headers":[]
            }])
        );
    } else {
        assert!(setup["params"]["mcpServers"].as_array().unwrap().is_empty());
    }
    assert!(setup["params"].get("additionalDirectories").is_none());
    if matches!(scenario.as_str(), "auth" | "rejected") {
        write_frame(
            &mut pipe,
            json!({"jsonrpc":"2.0", "id":setup["id"], "error":{
                "code":if scenario == "auth" {-32000} else {-32001},
                "message":"sensitive /private/provider/config", "data":{"credential":"must-not-escape"}
            }}),
        );
        std::thread::sleep(Duration::from_secs(60));
        return;
    }
    match setup["method"].as_str().unwrap() {
        "session/new" => write_frame(
            &mut pipe,
            json!({"jsonrpc":"2.0", "id":setup["id"], "result":{"sessionId":"opaque-fixture"}}),
        ),
        "session/load" => {
            assert_eq!(setup["params"]["sessionId"], "opaque-fixture");
            update(
                &mut pipe,
                "opaque-fixture",
                "agent_message_chunk",
                "旧历史不能重复输出",
            );
            write_frame(
                &mut pipe,
                json!({"jsonrpc":"2.0", "id":setup["id"], "result":{}}),
            );
        }
        method => panic!("unexpected setup: {method}"),
    }
    let prompt = read();
    assert_eq!(prompt["method"], "session/prompt");
    assert_eq!(prompt["params"]["sessionId"], "opaque-fixture");
    assert_eq!(
        prompt["params"]["prompt"],
        json!([{"type":"text", "text":"本次问题"}])
    );
    if scenario == "hang" {
        std::thread::sleep(Duration::from_secs(60));
        return;
    }
    if scenario == "mismatch" {
        update(
            &mut pipe,
            "another-session",
            "agent_message_chunk",
            "不能穿会话",
        );
        std::thread::sleep(Duration::from_secs(60));
        return;
    }
    if scenario == "malformed" {
        writeln!(pipe, "not-json").unwrap();
        pipe.flush().unwrap();
        std::thread::sleep(Duration::from_secs(60));
        return;
    }
    if scenario == "flood" {
        std::io::stderr().write_all(&vec![b'x'; 100_000]).unwrap();
        std::thread::sleep(Duration::from_secs(60));
        return;
    }
    if scenario == "permission" {
        write_frame(
            &mut pipe,
            json!({"jsonrpc":"2.0", "id":"permission", "method":"session/request_permission", "params":{
                "sessionId":"opaque-fixture", "toolCall":{"toolCallId":"write", "title":"不要批准", "status":"pending"},
                "options":[{"optionId":"allow", "name":"allow", "kind":"allow_once"}]
            }}),
        );
        let denied = read();
        assert_eq!(denied["id"], "permission");
        assert_eq!(denied["result"]["outcome"]["outcome"], "cancelled");
    }
    update(
        &mut pipe,
        "opaque-fixture",
        "agent_thought_chunk",
        "内部推理不能透传",
    );
    update(&mut pipe, "opaque-fixture", "agent_message_chunk", "实际");
    update(
        &mut pipe,
        "opaque-fixture",
        "agent_message_chunk",
        "答案\u{1b}\n",
    );
    write_frame(
        &mut pipe,
        json!({"jsonrpc":"2.0", "id":prompt["id"], "result":{"stopReason":"end_turn"}}),
    );
    std::thread::sleep(Duration::from_secs(60));
}

fn write_frame(pipe: &mut std::fs::File, frame: Value) {
    writeln!(pipe, "{frame}").unwrap();
    pipe.flush().unwrap();
}
fn update(pipe: &mut std::fs::File, session: &str, kind: &str, text: &str) {
    write_frame(
        pipe,
        json!({"jsonrpc":"2.0", "method":"session/update", "params":{"sessionId":session, "update":{"sessionUpdate":kind,"content":{"type":"text","text":text}}}}),
    );
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
fn actual_acp_turn_streams_only_current_agent_text_and_preserves_opaque_session() {
    for session in [None, Some("opaque-fixture")] {
        let fixture = fixture("normal");
        let config = Configuration::load(&fixture.config).unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let result = run_turn(
            &config,
            request(session),
            &AtomicBool::new(false),
            move |event| {
                captured
                    .lock()
                    .unwrap()
                    .push(serde_json::to_value(event).unwrap())
            },
        )
        .unwrap();
        assert_eq!(result.session_id.as_deref(), Some("opaque-fixture"));
        assert!(result.resumable);
        assert_eq!(result.text, "实际答案\n");
        assert_eq!(result.stop_reason, "end_turn");
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                json!({"kind":"session_started","sessionId":"opaque-fixture","resumable":true}),
                json!({"kind":"text_delta","text":"实际"}),
                json!({"kind":"text_delta","text":"答案\n"})
            ]
        );
        let calls = std::fs::read_to_string(fixture.root.join("turn.jsonl")).unwrap();
        for forbidden in [
            "authenticate",
            "modelId",
            "providers/set",
            "session/set_config_option",
        ] {
            assert!(!calls.contains(forbidden));
        }
        assert_reaped(&fixture);
    }
}

#[test]
fn http_mcp_servers_are_passed_to_new_and_loaded_sessions() {
    for (scenario, session) in [("mcp-new", None), ("mcp-load", Some("opaque-fixture"))] {
        let fixture = fixture(scenario);
        let config = Configuration::load(&fixture.config).unwrap();
        let result = run_turn(
            &config,
            request_with_mcp(session),
            &AtomicBool::new(false),
            |_| {},
        );
        assert!(
            result.is_ok(),
            "{result:?}; calls={}",
            std::fs::read_to_string(fixture.root.join("turn.jsonl")).unwrap_or_default()
        );
        let result = result.unwrap();
        assert_eq!(result.session_id.as_deref(), Some("opaque-fixture"));
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
            methods[1],
            if session.is_some() {
                "session/load"
            } else {
                "session/new"
            }
        );
        assert_eq!(methods[2], "session/prompt");
        assert_reaped(&fixture);
    }
}

#[test]
fn http_mcp_without_negotiated_capability_fails_before_session_or_prompt() {
    let fixture = fixture("mcp-unsupported");
    let config = Configuration::load(&fixture.config).unwrap();
    assert_eq!(
        run_turn(
            &config,
            request_with_mcp(None),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap_err(),
        AgentError::CapabilitiesUnsupported
    );
    let calls = std::fs::read_to_string(fixture.root.join("turn.jsonl")).unwrap();
    assert_eq!(calls.lines().count(), 1);
    assert_reaped(&fixture);
}

#[test]
fn undeclared_resume_fails_before_creating_or_prompting_a_session() {
    let fixture = fixture("no-load");
    let config = Configuration::load(&fixture.config).unwrap();
    assert_eq!(
        run_turn(
            &config,
            request(Some("opaque-fixture")),
            &AtomicBool::new(false),
            |_| {}
        )
        .unwrap_err(),
        AgentError::SessionUnsupported
    );
    let calls = std::fs::read_to_string(fixture.root.join("turn.jsonl")).unwrap();
    assert_eq!(calls.lines().count(), 1);
    assert_reaped(&fixture);
}

#[test]
fn permissions_are_cancelled_and_agent_requests_do_not_grant_host_tools() {
    let fixture = fixture("permission");
    let config = Configuration::load(&fixture.config).unwrap();
    assert_eq!(
        run_turn(&config, request(None), &AtomicBool::new(false), |_| {})
            .unwrap()
            .text,
        "实际答案\n"
    );
    assert_reaped(&fixture);
}

#[test]
fn wrong_session_malformed_protocol_and_output_flood_cannot_complete_turns() {
    for (scenario, expected) in [
        ("mismatch", AgentError::SessionMismatch),
        ("malformed", AgentError::ProtocolInvalid),
        ("flood", AgentError::TurnOutputLimit),
    ] {
        let fixture = fixture(scenario);
        let mut document = fixture.document();
        document["agents"][0]["adapter"] = json!("external_acp_adapter");
        document["turnTimeoutMs"] = json!(1500);
        document["maxTurnOutputBytes"] = json!(2048);
        fixture.save(document);
        let config = Configuration::load(&fixture.config).unwrap();
        assert_eq!(
            run_turn(&config, request(None), &AtomicBool::new(false), |_| {}).unwrap_err(),
            expected
        );
        assert_reaped(&fixture);
    }
}

#[test]
fn real_turn_cancellation_and_deadline_reap_the_process() {
    let fixture = fixture("hang");
    let config = Configuration::load(&fixture.config).unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        signal.store(true, Ordering::SeqCst);
    });
    let started = Instant::now();
    assert_eq!(
        run_turn(&config, request(None), &cancelled, |_| {}).unwrap_err(),
        AgentError::TurnCancelled
    );
    thread.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_reaped(&fixture);
    assert_eq!(
        run_turn(&config, request(None), &AtomicBool::new(false), |_| {}).unwrap_err(),
        AgentError::TurnTimedOut
    );
    assert_reaped(&fixture);
}

#[test]
fn invalid_request_and_unknown_agent_do_not_start_an_agent() {
    let fixture = fixture("normal");
    let config = Configuration::load(&fixture.config).unwrap();
    let mut invalid = request(None);
    invalid.prompt.clear();
    assert_eq!(
        run_turn(&config, invalid, &AtomicBool::new(false), |_| {}).unwrap_err(),
        AgentError::RequestInvalid
    );
    let mut unknown = request(None);
    unknown.agent_id = "missing".into();
    assert_eq!(
        run_turn(&config, unknown, &AtomicBool::new(false), |_| {}).unwrap_err(),
        AgentError::RequestInvalid
    );
    assert!(!fixture.root.join("turn.jsonl").exists());
}

#[test]
fn connection_scoped_sessions_can_complete_without_claiming_cross_connection_resume() {
    let fixture = fixture("connection");
    let config = Configuration::load(&fixture.config).unwrap();
    let result = run_turn(&config, request(None), &AtomicBool::new(false), |_| {}).unwrap();
    assert!(!result.resumable);
    assert_eq!(result.text, "实际答案\n");
    assert_reaped(&fixture);
}

#[test]
fn turn_limits_are_validated_before_any_process_is_started() {
    for (key, value) in [
        ("turnTimeoutMs", 0),
        ("turnTimeoutMs", 600001),
        ("maxTurnOutputBytes", 127),
        ("maxTurnOutputBytes", 4194305),
    ] {
        let fixture = fixture("normal");
        let mut document = fixture.document();
        document[key] = json!(value);
        fixture.save(document);
        assert_eq!(
            Configuration::load(&fixture.config).unwrap_err(),
            AgentError::ConfigurationInvalid
        );
        assert!(!fixture.root.join("turn.jsonl").exists());
    }
}

#[test]
fn formal_authentication_error_does_not_trigger_login_or_expose_remote_error_data() {
    for (scenario, expected) in [
        ("auth", AgentError::AuthenticationRequired),
        ("rejected", AgentError::AgentRequestFailed),
    ] {
        let fixture = fixture(scenario);
        let config = Configuration::load(&fixture.config).unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let error = run_turn(
            &config,
            request(None),
            &AtomicBool::new(false),
            move |event| captured.lock().unwrap().push(event),
        )
        .unwrap_err();
        assert_eq!(error, expected);
        assert!(events.lock().unwrap().is_empty());
        assert!(!serde_json::to_string(&error).unwrap().contains("sensitive"));
        let calls = std::fs::read_to_string(fixture.root.join("turn.jsonl")).unwrap();
        assert!(!calls.contains("authenticate"));
        assert!(!calls.contains("session/prompt"));
        assert_reaped(&fixture);
    }
}

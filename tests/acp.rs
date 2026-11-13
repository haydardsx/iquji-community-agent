#![cfg(unix)]
mod support;

use iquji_shared_agent::{Configuration, scan};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::os::fd::FromRawFd;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use support::{Fixture, quote};

fn fixture(adapter: &str, scenario: &str) -> Fixture {
    let fixture = Fixture::new();
    fixture.script(&format!(
        "SHARED_AGENT_ACP_FIXTURE='{scenario}' SHARED_AGENT_ACP_RECORD={} exec {} --exact acp_peer_fixture --nocapture 3>&1 1>/dev/null",
        quote(&fixture.root.join("request.json")), quote(&std::env::current_exe().unwrap()),
    ));
    let mut document = fixture.document();
    document["agents"][0]["adapter"] = json!(adapter);
    document["timeoutMs"] = json!(1000);
    fixture.save(document);
    fixture
}

#[test]
fn acp_peer_fixture() {
    let Ok(scenario) = std::env::var("SHARED_AGENT_ACP_FIXTURE") else {
        return;
    };
    let mut pipe = unsafe { std::fs::File::from_raw_fd(3) };
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).unwrap();
    let request: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(request["method"], "initialize");
    assert_eq!(request["params"]["protocolVersion"], 1);
    assert_ne!(request["params"]["clientCapabilities"]["terminal"], true);
    assert_ne!(
        request["params"]["clientCapabilities"]["fs"]["writeTextFile"],
        true
    );
    for name in [
        "HOME",
        "CODEX_HOME",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "HTTPS_PROXY",
    ] {
        assert!(std::env::var_os(name).is_none());
    }
    let record = std::path::PathBuf::from(std::env::var_os("SHARED_AGENT_ACP_RECORD").unwrap());
    std::fs::write(&record, &line).unwrap();
    std::fs::write(record.with_extension("pid"), std::process::id().to_string()).unwrap();
    match scenario.as_str() {
        "hang" => std::thread::sleep(Duration::from_secs(60)),
        "malformed" => writeln!(pipe, "untrusted non-json protocol").unwrap(),
        "flood" => {
            pipe.write_all(&vec![b'x'; 100_000]).unwrap();
        }
        "stderr-flood" => {
            std::io::stderr().write_all(&vec![b'x'; 100_000]).unwrap();
        }
        _ => {
            let id = if scenario == "wrong-id" {
                json!("unknown-request")
            } else {
                request["id"].clone()
            };
            let response = json!({"jsonrpc":"2.0", "id":id, "result":{
                "protocolVersion":if scenario == "unsupported" {2} else {1},
                "agentCapabilities":{"loadSession":scenario == "loadable", "promptCapabilities":{"image":scenario == "loadable"}},
                "authMethods":[{"id":"provider-auth", "name":"Do not sign in during scan"}],
                "agentInfo":{"name":"untrusted", "version":"/private/fixture/path?secret=hidden"}
            }});
            writeln!(pipe, "{response}").unwrap();
        }
    }
    pipe.flush().unwrap();
    std::thread::sleep(Duration::from_secs(60));
}

#[test]
fn native_acp_and_external_adapter_use_real_v1_negotiation_without_session_or_login() {
    for (adapter, scenario, mode, formats) in [
        (
            "native_acp",
            "loadable",
            "resumable",
            json!(["image/png", "image/jpeg"]),
        ),
        (
            "external_acp_adapter",
            "connection",
            "connection_scoped",
            json!([]),
        ),
    ] {
        let fixture = fixture(adapter, scenario);
        let report = fixture.report();
        let connection = &report["connections"][0];
        assert_eq!(connection["status"], "detected");
        assert_eq!(connection["adapter"], adapter);
        assert_eq!(connection["protocolVersion"], 1);
        assert_eq!(connection["version"], Value::Null);
        assert_eq!(connection["sessionMode"], mode);
        assert_eq!(connection["supportedInputFormats"], formats);
        assert_eq!(report["readiness"], "not_checked");
        assert!(!report.to_string().contains("hidden"));
        let request: Value =
            serde_json::from_slice(&std::fs::read(fixture.root.join("request.json")).unwrap())
                .unwrap();
        assert_eq!(request["method"], "initialize");
        assert_reaped(&fixture);
    }
}

#[test]
fn malformed_version_mismatch_and_wrong_response_identity_do_not_report_detection() {
    for (scenario, expected) in [
        ("malformed", "protocol_invalid"),
        ("unsupported", "protocol_unsupported"),
        ("wrong-id", "probe_timed_out"),
    ] {
        fixture("native_acp", scenario).assert_error(expected);
    }
}

#[test]
fn acp_stdout_and_stderr_share_a_hard_byte_limit() {
    for scenario in ["flood", "stderr-flood"] {
        let fixture = fixture("native_acp", scenario);
        let mut config = fixture.document();
        config["agents"][0]["adapter"] = json!("native_acp");
        config["maxOutputBytes"] = json!(256);
        fixture.save(config);
        fixture.assert_error("probe_output_limit");
    }
}

#[test]
fn native_cancellation_and_timeout_reap_the_negotiating_process() {
    let fixture = fixture("native_acp", "hang");
    let mut document = fixture.document();
    document["agents"][0]["adapter"] = json!("native_acp");
    document["timeoutMs"] = json!(5000);
    fixture.save(document);
    let config = Configuration::load(&fixture.config).unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    let pid_file = fixture.root.join("request.pid");
    let thread = std::thread::spawn(move || {
        // 等待协商进程写出身份后再取消，避免繁忙运行器在进程启动前收到信号。
        let deadline = Instant::now() + Duration::from_secs(4);
        while !std::fs::read_to_string(&pid_file).is_ok_and(|pid| pid.parse::<i32>().is_ok()) {
            assert!(Instant::now() < deadline, "协商进程未在期限内启动");
            std::thread::sleep(Duration::from_millis(10));
        }
        let signalled_at = Instant::now();
        signal.store(true, Ordering::SeqCst);
        signalled_at
    });
    let report = serde_json::to_value(scan(&config, &cancelled)).unwrap();
    let signalled_at = thread.join().unwrap();
    assert_eq!(report["connections"][0]["error"], "probe_cancelled");
    assert!(signalled_at.elapsed() < Duration::from_secs(2));
    assert_reaped(&fixture);
    // 第二次探测必须记录新进程，不能复用已回收进程的身份通过断言。
    std::fs::remove_file(fixture.root.join("request.pid")).unwrap();
    fixture.assert_error("probe_timed_out");
    assert_reaped(&fixture);
}

fn assert_reaped(fixture: &Fixture) {
    let pid: i32 = std::fs::read_to_string(fixture.root.join("request.pid"))
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
fn embedding_hosts_cannot_capture_raw_sdk_protocol_traces() {
    use std::sync::atomic::AtomicUsize;
    use tracing::{
        Metadata, Subscriber,
        span::{Attributes, Id, Record},
    };
    struct HostLog(Arc<AtomicUsize>);
    impl Subscriber for HostLog {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            self.0.fetch_add(1, Ordering::SeqCst);
            Id::from_u64(1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, _: &tracing::Event<'_>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }
    let fixture = fixture("native_acp", "loadable");
    let config = Configuration::load(&fixture.config).unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    tracing::subscriber::with_default(HostLog(count.clone()), || {
        tracing::info!("host_before_scan");
        let report = serde_json::to_value(scan(&config, &AtomicBool::new(false))).unwrap();
        assert_eq!(report["connections"][0]["status"], "detected");
        tracing::info!("host_after_scan");
    });
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

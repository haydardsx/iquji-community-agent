use super::{Fixture, quote};
use iquji_shared_agent::{AgentError, Configuration, TurnEvent, TurnRequest, TurnResult, run_turn};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex, atomic::AtomicBool};
use std::time::{Duration, Instant};

pub fn fixture(scenario: &str, timeout: u64, limit: usize) -> Fixture {
    let fixture = Fixture::new();
    let protected = super::CONFIGURATION_OVERRIDES
        .iter()
        .map(|name| format!("${{{name}+x}}"))
        .collect::<String>();
    fixture.script(&format!(r#"
if [ "{protected}" != "" ]; then exit 45; fi
case "$*" in
 --version|'exec --help'|'exec resume --help')
   printf '%s\t%s\t%s\t%s\n' "$HOME" "$CODEX_HOME" "${{FAKE_PROVIDER_KEY-unset}}" "${{OPENAI_API_KEY-unset}}" >> {metadata}
   case "$*" in
    --version) printf 'codex-cli 0.50.0\n';;
    'exec --help') printf '%s\n' '--json --color --sandbox --skip-git-repo-check -C resume';;
    *) printf '%s\n' '--json';;
   esac;;
 *) printf '%s\000' "$@" > {arguments}
    SHARED_CODEX_SCENARIO='{scenario}' SHARED_CODEX_RECORD={record} exec {peer} --exact codex_peer_fixture --nocapture 3>&1 1>/dev/null;;
esac
"#, metadata=quote(&fixture.root.join("metadata")), arguments=quote(&fixture.root.join("arguments")), record=quote(&fixture.root.join("record")), peer=quote(&std::env::current_exe().unwrap())));
    let mut config = fixture.document();
    config["timeoutMs"] = json!(1000);
    config["turnTimeoutMs"] = json!(timeout);
    config["maxTurnOutputBytes"] = json!(limit);
    fixture.save(config);
    fixture
}

pub fn request(session: Option<&str>) -> TurnRequest {
    TurnRequest {
        workspace_write: false,
        agent_id: "fixture".into(),
        context_fingerprint: None,
        prompt: "问题 ; $(printf unexpected)\n文本".into(),
        session_id: session.map(str::to_owned),
        model: None,
        attachments: Vec::new(),
        mcp_servers: Vec::new(),
    }
}
pub fn invoke(
    fixture: &Fixture,
    session: Option<&str>,
    cancelled: &AtomicBool,
) -> (Result<TurnResult, AgentError>, Vec<Value>) {
    invoke_request(fixture, request(session), cancelled)
}

pub fn invoke_request(
    fixture: &Fixture,
    request: TurnRequest,
    cancelled: &AtomicBool,
) -> (Result<TurnResult, AgentError>, Vec<Value>) {
    let configuration = Configuration::load(&fixture.config).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let collected = events.clone();
    let result = run_turn(
        &configuration,
        request,
        cancelled,
        move |event: TurnEvent| {
            collected
                .lock()
                .unwrap()
                .push(serde_json::to_value(event).unwrap());
        },
    );
    (result, events.lock().unwrap().clone())
}
pub fn command(fixture: &Fixture) -> Command {
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
pub fn cli(fixture: &Fixture, extra_env: &[(&str, &std::ffi::OsStr)]) -> Output {
    let mut command = command(fixture);
    for (name, value) in extra_env {
        command.env(name, value);
    }
    let mut child = command.spawn().unwrap();
    let payload = serde_json::to_vec(&json!({"agentId":"fixture","prompt":"本次问题"})).unwrap();
    if let Err(error) = child.stdin.take().unwrap().write_all(&payload) {
        // CLI 在读取 stdin 前拒绝配置时合法退出，写入会得到 BrokenPipe。
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }
    bounded(child)
}
pub fn bounded(mut child: Child) -> Output {
    let end = Instant::now() + Duration::from_secs(8);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > end {
            let _ = child.kill();
            panic!("假CLI未有界退出");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    child.wait_with_output().unwrap()
}
pub fn frames(output: &Output) -> Vec<Value> {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
pub fn wait_file(path: &std::path::Path) {
    let end = Instant::now() + Duration::from_secs(4);
    while !path.exists() {
        assert!(Instant::now() < end, "假CLI未就绪");
        std::thread::sleep(Duration::from_millis(5));
    }
}
pub fn record(fixture: &Fixture) -> Value {
    serde_json::from_slice(&std::fs::read(fixture.root.join("record")).unwrap()).unwrap()
}
pub fn assert_cleaned(fixture: &Fixture) {
    let metadata = std::fs::read_to_string(fixture.root.join("metadata")).unwrap();
    for line in metadata.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        assert_eq!(fields[0], fields[1]);
        assert_eq!(&fields[2..], &["unset", "unset"]);
        assert!(
            !std::path::Path::new(fields[0]).exists(),
            "metadata目录未清理"
        );
    }
    if fixture.root.join("record").exists() {
        let record = record(fixture);
        for name in ["home", "codexHome"] {
            if !std::path::Path::new(record[name].as_str().unwrap()).starts_with(&fixture.root) {
                assert!(
                    !std::path::Path::new(record[name].as_str().unwrap()).exists(),
                    "执行目录未清理"
                );
            }
        }
    }
}
pub fn assert_reaped(fixture: &Fixture) {
    let pid = record(fixture)["pid"].as_i64().unwrap() as i32;
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}
fn frame(pipe: &mut std::fs::File, value: Value) {
    serde_json::to_writer(&mut *pipe, &value).unwrap();
    pipe.write_all(b"\n").unwrap();
    pipe.flush().unwrap();
}
pub fn peer() {
    let Ok(scenario) = std::env::var("SHARED_CODEX_SCENARIO") else {
        return;
    };
    let path = std::path::PathBuf::from(std::env::var_os("SHARED_CODEX_RECORD").unwrap());
    super::assert_configuration_overrides_absent();
    let home = std::env::var("HOME").unwrap();
    let codex_home = std::env::var("CODEX_HOME").unwrap();
    let codex_directory = std::path::Path::new(&codex_home);
    std::fs::write(&path,serde_json::to_vec(&json!({"pid":std::process::id(),"home":home,"codexHome":codex_home,"homeMode":std::fs::metadata(&home).unwrap().permissions().mode() & 0o777,"codexMode":std::fs::metadata(&codex_home).unwrap().permissions().mode() & 0o777,"codexConfig":std::fs::read_to_string(codex_directory.join("config.toml")).ok(),"authIsSymlink":std::fs::symlink_metadata(codex_directory.join("auth.json")).is_ok_and(|metadata|metadata.file_type().is_symlink()),"sessionsIsSymlink":std::fs::symlink_metadata(codex_directory.join("sessions")).is_ok_and(|metadata|metadata.file_type().is_symlink()),"provider":std::env::var("FAKE_PROVIDER_KEY").ok(),"openai":std::env::var("OPENAI_API_KEY").ok(),"proxy":std::env::var("HTTP_PROXY").ok()})).unwrap()).unwrap();
    let mut pipe = unsafe { std::fs::File::from_raw_fd(3) };
    if scenario == "no-read" {
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    let mut prompt = Vec::new();
    std::io::stdin().read_to_end(&mut prompt).unwrap();
    std::fs::write(path.with_extension("prompt"), prompt).unwrap();
    if scenario == "malformed" {
        pipe.write_all(b"not json\n").unwrap();
        return;
    }
    if scenario == "credential" {
        frame(
            &mut pipe,
            json!({"type":"error","message":"Missing API key sensitive-private"}),
        );
        return;
    }
    if scenario == "failed" {
        frame(
            &mut pipe,
            json!({"type":"turn.failed","error":{"message":"sensitive-private"}}),
        );
        return;
    }
    if scenario == "order" {
        frame(&mut pipe, json!({"type":"turn.started"}));
        return;
    }
    let thread = if scenario == "mismatch" {
        "other-thread"
    } else {
        "thread-fixture"
    };
    frame(
        &mut pipe,
        json!({"type":"thread.started","thread_id":thread}),
    );
    if scenario == "duplicate" {
        frame(
            &mut pipe,
            json!({"type":"thread.started","thread_id":thread}),
        );
        return;
    }
    if scenario == "startup-diagnostics" {
        for _ in 0..2 {
            frame(
                &mut pipe,
                json!({"type":"item.completed","item":{"id":"diagnostic","type":"error","message":"sensitive-private diagnostic"}}),
            );
        }
    }
    if scenario == "startup-assistant" {
        frame(
            &mut pipe,
            json!({"type":"item.completed","item":{"type":"agent_message","text":"premature answer"}}),
        );
    }
    frame(&mut pipe, json!({"type":"turn.started"}));
    if scenario == "hang" {
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    if scenario == "flood" {
        loop {
            std::io::stderr().write_all(&[b'x'; 1024]).unwrap();
        }
    }
    if scenario == "partial" {
        return;
    }
    if scenario != "no-assistant" {
        frame(
            &mut pipe,
            json!({"type":"item.completed","item":{"type":"reasoning","text":"private reasoning"}}),
        );
        frame(
            &mut pipe,
            json!({"type":"item.completed","item":{"type":"agent_message","text":"较早消息"}}),
        );
        frame(
            &mut pipe,
            json!({"type":"item.completed","item":{"type":"command_execution","output":"private tool"}}),
        );
        let text = if scenario == "large" {
            "答".repeat(20_000)
        } else {
            "正式回答\n\t结束\u{001b}".into()
        };
        frame(
            &mut pipe,
            json!({"type":"item.completed","item":{"type":"agent_message","text":text}}),
        );
    }
    frame(&mut pipe, json!({"type":"turn.completed","usage":{}}));
    std::io::stderr()
        .write_all(b"sensitive-private stderr")
        .unwrap();
    if scenario == "detach" {
        use std::os::unix::process::CommandExt;
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "codex_detached_fixture", "--nocapture"])
            .env("SHARED_CODEX_DETACHED", path.with_extension("detached"));
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        // 测试故意让入口先退出；脱组后代由外层测试按其真实 PID 清理。
        #[allow(clippy::zombie_processes)]
        let _child = command.spawn().unwrap();
        wait_file(&path.with_extension("detached"));
    }
}

pub fn detached_peer() {
    let Some(path) = std::env::var_os("SHARED_CODEX_DETACHED") else {
        return;
    };
    std::fs::write(path, std::process::id().to_string()).unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#![cfg(unix)]
mod support;
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use support::{Fixture, private_mode, quote};

fn directory(fixture: &Fixture, name: &str) -> std::path::PathBuf {
    let result = fixture.root.join(name);
    std::fs::create_dir(&result).unwrap();
    private_mode(&result, 0o700);
    result
}
fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    private_mode(path, 0o600);
}
fn command(fixture: &Fixture, kind: &str) -> std::process::Command {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_iquji-shared-agent"));
    command
        .args(["--config", fixture.config.to_str().unwrap(), kind])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(&fixture.root);
    command
}
fn inspect(fixture: &Fixture) -> Value {
    let output = command(fixture, "inspect")
        .env("FAKE_PROVIDER_KEY", "fake-secret-marker")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("fake-secret-marker"));
    assert!(!text.contains(fixture.root.to_str().unwrap()));
    serde_json::from_str(&text).unwrap()
}

fn expected_context(fixture: &Fixture, adapter: iquji_agent_wire::Adapter, home: &Path) -> String {
    use sha2::{Digest, Sha256};
    let digest = format!(
        "{:x}",
        Sha256::digest(std::fs::read(&fixture.program).unwrap())
    );
    iquji_agent_wire::inspection_context_fingerprint(
        adapter,
        &digest,
        home.to_str().unwrap(),
        if adapter == iquji_agent_wire::Adapter::DeepseekHarness {
            fixture.workspace.to_str()
        } else {
            None
        },
    )
    .unwrap()
}
fn turn(fixture: &Fixture, adapter: &str, resume: bool) -> std::process::Output {
    let mut child = command(fixture, "turn")
        .env("FAKE_PROVIDER_KEY", "fake-key")
        .env("FAKE_ROUTE_KEY", "fake-route")
        .env("FAKE_DSH_KEY", "fake-dsh")
        .env("NOT_SELECTED", "not-for-child")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let request = json!({"agentId":"fixture","prompt":"ask","model":"relay/model-v2","sessionId":if resume {json!("real-thread")} else {Value::Null}});
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{adapter}: {} / {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    output
}
fn turn_request(fixture: &Fixture, request: Value) -> std::process::Output {
    let mut child = command(fixture, "turn")
        .env("FAKE_DSH_KEY", "fake-dsh")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    child.wait_with_output().unwrap()
}
fn mcp_request(model: Option<&str>, name: &str) -> Value {
    let mut request = json!({
        "agentId": "fixture",
        "prompt": "ask",
        "mcpServers": [{
            "name": name,
            "url": "http://127.0.0.1:43123/mcp/0123456789abcdef0123456789abcdef0123456789abcdef"
        }]
    });
    if let Some(model) = model {
        request["model"] = json!(model);
    }
    request
}
fn codex() -> Fixture {
    let fixture = Fixture::new();
    let home = directory(&fixture, "cli-home");
    write(
        &home.join("config.toml"),
        r#"model_provider='relay'
model='relay/default'
model_catalog_json='../catalog.json'
[model_providers.relay]
base_url='https://relay.invalid/secret-not-result'
env_key='FAKE_PROVIDER_KEY'
env_http_headers={ 'X-Route-Key'='FAKE_ROUTE_KEY' }
[model_providers.other]
env_key='OTHER_KEY'
"#,
    );
    write(
        &fixture.root.join("catalog.json"),
        r#"{"models":[{"slug":"relay/model-v2","visibility":"list"},{"slug":"private-hidden","visibility":"hide"}]}"#,
    );
    let mut config = fixture.document();
    config["agents"][0]["codexConfigurationDirectory"] = json!(home);
    config["turnTimeoutMs"] = json!(3000);
    fixture.save(config);
    fixture.script(&format!(r#"
case "$*" in
--version) printf 'codex-cli 0.50.0\n';;
'exec --help') [ "${{FAKE_PROVIDER_KEY+x}}" = '' ] || exit 31; printf '%s\n' '--json --color --sandbox --skip-git-repo-check -C resume --image --model';;
'exec resume --help') printf '%s\n' '--image';;
*) printf '%s\000' "$@" > {args}; printf '%s\n' "${{FAKE_PROVIDER_KEY-unset}}" "${{FAKE_ROUTE_KEY-unset}}" "${{NOT_SELECTED-unset}}" > {environment}; cat >/dev/null; printf '%s\n' '{{"type":"thread.started","thread_id":"real-thread"}}' '{{"type":"turn.started"}}' '{{"type":"item.completed","item":{{"type":"agent_message","text":"model reply"}}}}' '{{"type":"turn.completed"}}';;
esac
"#,args=quote(&fixture.root.join("arguments")),environment=quote(&fixture.root.join("environment"))));
    fixture
}

#[test]
fn 显式检查保留中转提供方型号与凭据名称而不输出配置内容() {
    let fixture = codex();
    let report = inspect(&fixture);
    assert_eq!(report["readiness"], "not_checked");
    assert_eq!(
        report["agents"][0]["inspectionContextFingerprint"],
        expected_context(
            &fixture,
            iquji_agent_wire::Adapter::CodexCli,
            &fixture.root.join("cli-home")
        )
    );
    let metadata = &report["agents"][0]["metadata"];
    assert_eq!(metadata["provider"], "relay");
    assert_eq!(metadata["configuredModel"], "relay/default");
    assert_eq!(
        metadata["supportedModels"],
        json!(["relay/default", "relay/model-v2"])
    );
    assert_eq!(
        metadata["credentialEnvironmentVariables"],
        json!(["FAKE_PROVIDER_KEY", "FAKE_ROUTE_KEY"])
    );
    assert!(!report.to_string().contains("relay.invalid"));
    let previous = metadata["configurationFingerprint"].clone();
    write(
        &fixture.root.join("catalog.json"),
        r#"{"models":[{"slug":"relay/new-model"}]}"#,
    );
    assert_ne!(
        inspect(&fixture)["agents"][0]["metadata"]["configurationFingerprint"],
        previous
    );
    let path = fixture.root.join("cli-home/config.toml");
    let source = std::fs::read_to_string(&path)
        .unwrap()
        .replace("model_provider='relay'", "model_provider='中转甲'")
        .replace("[model_providers.relay]", "[model_providers.'中转甲']");
    write(&path, &source);
    assert_eq!(
        inspect(&fixture)["agents"][0]["metadata"]["provider"],
        "中转甲"
    );
    assert!(!fixture.calls.exists());
}

#[test]
fn 未声明配置不读取环境目录且第三方不混用官方全局缓存() {
    let fixture = codex();
    let home = fixture.root.join("cli-home");
    write(
        &home.join("config.toml"),
        "model_provider='relay'\nmodel='relay/default'\n",
    );
    write(
        &home.join("models_cache.json"),
        r#"{"models":[{"slug":"official-unrelated"}]}"#,
    );
    assert_eq!(
        inspect(&fixture)["agents"][0]["metadata"]["supportedModels"],
        json!(["relay/default"])
    );
    let mut config = fixture.document();
    config["agents"][0]
        .as_object_mut()
        .unwrap()
        .remove("codexConfigurationDirectory");
    fixture.save(config);
    let output = command(&fixture, "inspect")
        .env("HOME", &home)
        .env("CODEX_HOME", &home)
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["agents"][0]["error"],
        "configuration_metadata_unavailable"
    );
    assert!(report["agents"][0]["metadata"].is_null());
    assert!(report["agents"][0]["inspectionContextFingerprint"].is_null());
}

#[path = "configuration/context.rs"]
mod context_tests;
#[path = "configuration/preparation.rs"]
mod preparation_tests;

#[test]
fn 明确引用的目录外文件不安全或缺失时不切换默认提供方() {
    use std::os::unix::fs::symlink;
    for case in [
        "missing",
        "symlink",
        "writable",
        "directory",
        "large",
        "auth",
        "fifo",
    ] {
        let fixture = codex();
        let path = fixture.root.join("catalog.json");
        match case {
            "missing" => std::fs::remove_file(&path).unwrap(),
            "symlink" => {
                std::fs::remove_file(&path).unwrap();
                write(&fixture.root.join("target.json"), "{}");
                symlink(fixture.root.join("target.json"), &path).unwrap();
            }
            "writable" => private_mode(&path, 0o666),
            "directory" => {
                std::fs::remove_file(&path).unwrap();
                std::fs::create_dir(&path).unwrap();
            }
            "large" => write(&path, &"x".repeat(1_048_577)),
            "fifo" => {
                std::fs::remove_file(&path).unwrap();
                let name = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            }
            "auth" => {
                write(
                    &fixture.root.join("cli-home/config.toml"),
                    "model_provider='relay'\nmodel_catalog_json='../auth.json'\n",
                );
                write(&fixture.root.join("auth.json"), "{}");
            }
            _ => unreachable!(),
        }
        let report = inspect(&fixture);
        assert_eq!(
            report["agents"][0]["error"], "configuration_metadata_unavailable",
            "{case}"
        );
        assert!(report["agents"][0]["metadata"].is_null());
    }
}

#[test]
fn codex型号独立argv在首轮与续聊保留选中配置与环境引用() {
    for resume in [false, true] {
        let fixture = codex();
        let source = std::fs::read(fixture.root.join("cli-home/config.toml")).unwrap();
        let output = turn(&fixture, "codex", resume);
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains("model reply")
        );
        let bytes = std::fs::read(fixture.root.join("arguments")).unwrap();
        let args = bytes
            .split(|b| *b == 0)
            .filter(|a| !a.is_empty())
            .map(|a| String::from_utf8(a.to_vec()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(&args[..3], ["exec", "-m", "relay/model-v2"]);
        assert_eq!(args.iter().any(|a| a == "resume"), resume);
        assert_eq!(
            std::fs::read_to_string(fixture.root.join("environment")).unwrap(),
            "fake-key\nfake-route\nunset\n"
        );
        assert_eq!(
            std::fs::read(fixture.root.join("cli-home/config.toml")).unwrap(),
            source
        );
    }
}

fn dsh(sleep: bool) -> Fixture {
    let fixture = Fixture::new();
    let home = directory(&fixture, "dsh-home");
    let settings = fixture.root.join("external-settings.json");
    write(&settings,&json!({"agent-default-model":{"provider":"relay","model":"relay/default"},"llm-pi-ai":{"providers":{"relay":{"apiKeyEnv":"FAKE_DSH_KEY","baseUrl":"https://relay.invalid/do-not-output","models":[{"id":"relay/model-v2"}]}}},"private-setting":"fake-secret-marker"}).to_string());
    write(&home.join("dump.json"),&json!([{"id":"model-selection","name":"@deepseek-ai/dsh-agent-default-model","config":{"provider":"deepseek-official","model":"official-old"}},{"id":"settings","name":"@deepseek-ai/dsh-settings-file","config":{"path":settings}},{"id":"provider","name":"@deepseek-ai/dsh-llm-pi-ai","config":{"providers":{}}}]).to_string());
    let mut config = fixture.document();
    config["agents"][0]["adapter"] = json!("deepseek_harness");
    config["agents"][0]["deepseekConfigurationDirectory"] = json!(home);
    config["turnTimeoutMs"] = json!(if sleep { 1200 } else { 3000 });
    fixture.save(config);
    fixture.script(&format!(r#"
case "$*" in
--version) printf '0.1.0-rc.6\n';;
--help) printf '%s\n' '--profile --patch --dump-config';;
'--profile headless --dump-config') [ "${{FAKE_DSH_KEY+x}}" = '' ] || exit 32; [ "$DSH_PERMISSION_MODE" = 'read-only' ] || exit 33; cat "$DSH_HOME/dump.json" 2>/dev/null || cat {dump};;
*) MODEL_PATCH="$4" MODEL_RECORD={record} MODEL_SLEEP={sleep} exec {peer} --exact model_peer_fixture --nocapture 3>&1 1>/dev/null;;
esac
"#,dump=quote(&home.join("dump.json")),record=quote(&fixture.root.join("model-record.json")),sleep=if sleep{"yes"}else{"no"},peer=quote(&std::env::current_exe().unwrap())));
    fixture
}
#[test]
fn model_peer_fixture() {
    let Some(record) = std::env::var_os("MODEL_RECORD") else {
        return;
    };
    let path = std::path::PathBuf::from(std::env::var_os("MODEL_PATCH").unwrap());
    let patch: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let settings_path = patch
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == "settings")
        .and_then(|entry| entry["config"]["path"].as_str())
        .map(std::path::PathBuf::from);
    let settings = settings_path
        .as_ref()
        .map(|path| serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap())
        .unwrap_or_else(|| json!({}));
    let attack_settings = settings_path
        .clone()
        .unwrap_or_else(|| path.parent().unwrap().join("settings.json"));
    std::fs::write(record,json!({"patchPath":path,"settingsPath":settings_path,"patch":patch,"settings":settings,"key":std::env::var("FAKE_DSH_KEY").ok(),"mode":std::env::var("DSH_PERMISSION_MODE").ok(),"dshHome":std::env::var("DSH_HOME").ok()}).to_string()).unwrap();
    turn_binding_tests::mutate_if_requested(&path, &attack_settings);
    if std::env::var("MODEL_SLEEP").unwrap() == "yes" {
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
    use std::os::fd::FromRawFd;
    let mut output = unsafe { std::fs::File::from_raw_fd(3) };
    output.write_all(b"model reply").unwrap();
}

#[path = "configuration/turn_binding.rs"]
mod turn_binding_tests;
#[test]
fn dsh使用正式合成入口和临时覆盖保留提供方且不改原设置() {
    let fixture = dsh(false);
    let source = std::fs::read(fixture.root.join("external-settings.json")).unwrap();
    let report = inspect(&fixture);
    assert_eq!(
        report["agents"][0]["inspectionContextFingerprint"],
        expected_context(
            &fixture,
            iquji_agent_wire::Adapter::DeepseekHarness,
            &fixture.root.join("dsh-home")
        )
    );
    let metadata = &report["agents"][0]["metadata"];
    assert_eq!(metadata["provider"], "relay");
    assert_eq!(metadata["configuredModel"], "relay/default");
    assert_eq!(
        metadata["credentialEnvironmentVariables"],
        json!(["FAKE_DSH_KEY"])
    );
    let output = turn(&fixture, "dsh", false);
    let frames = String::from_utf8(output.stdout).unwrap();
    assert!(!frames.contains("session_started"));
    assert!(!frames.contains("sessionId"));
    let record: Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join("model-record.json")).unwrap())
            .unwrap();
    assert_eq!(
        record["patch"][0]["config"],
        json!({"provider":"relay","model":"relay/model-v2"})
    );
    assert_eq!(
        record["settings"]["llm-pi-ai"]["providers"]["relay"]["baseUrl"],
        "https://relay.invalid/do-not-output"
    );
    assert_eq!(
        record["settings"]["agent-default-model"]["model"],
        "relay/model-v2"
    );
    assert_eq!(record["key"], "fake-dsh");
    assert_eq!(record["mode"], "read-only");
    assert!(!Path::new(record["patchPath"].as_str().unwrap()).exists());
    assert!(!Path::new(record["settingsPath"].as_str().unwrap()).exists());
    assert_eq!(
        std::fs::read(fixture.root.join("external-settings.json")).unwrap(),
        source
    );
}

#[test]
fn dsh以临时insert注入本回合mcp且不改型号或用户配置() {
    let fixture = dsh(false);
    let original = std::fs::read(fixture.root.join("external-settings.json")).unwrap();
    let output = turn_request(&fixture, mcp_request(None, "docs"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record: Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join("model-record.json")).unwrap())
            .unwrap();
    assert_eq!(record["patch"].as_array().unwrap().len(), 1);
    assert_eq!(
        record["patch"][0]["insert"][0],
        json!({
            "id": "runtime-mcp-docs",
            "name": "@deepseek-ai/dsh-mcp-client",
            "config": {
                "serverName": "docs",
                "transport": "streamable-http",
                "url": "http://127.0.0.1:43123/mcp/0123456789abcdef0123456789abcdef0123456789abcdef",
                "headers": {}
            }
        })
    );
    assert!(record["settingsPath"].is_null());
    assert_eq!(record["mode"], "read-only");
    assert!(!Path::new(record["patchPath"].as_str().unwrap()).exists());
    assert_eq!(
        std::fs::read(fixture.root.join("external-settings.json")).unwrap(),
        original
    );
}

#[test]
fn dsh无显式配置时使用隔离目录注入mcp且不继承凭据() {
    let fixture = dsh(false);
    let mut config: Value =
        serde_json::from_slice(&std::fs::read(&fixture.config).unwrap()).unwrap();
    config["agents"][0]
        .as_object_mut()
        .unwrap()
        .remove("deepseekConfigurationDirectory");
    fixture.save(config);
    let output = turn_request(&fixture, mcp_request(None, "docs"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record: Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join("model-record.json")).unwrap())
            .unwrap();
    assert!(record["key"].is_null());
    assert!(!Path::new(record["patchPath"].as_str().unwrap()).exists());
    assert!(!Path::new(record["dshHome"].as_str().unwrap()).exists());
}

#[test]
fn dsh型号覆盖和mcp共用临时patch且同名服务或id冲突拒绝执行() {
    let fixture = dsh(false);
    let original = std::fs::read(fixture.root.join("external-settings.json")).unwrap();
    let output = turn_request(&fixture, mcp_request(Some("relay/model-v2"), "docs"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record: Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join("model-record.json")).unwrap())
            .unwrap();
    assert_eq!(record["patch"].as_array().unwrap().len(), 3);
    assert_eq!(
        record["patch"][0]["config"],
        json!({"provider":"relay","model":"relay/model-v2"})
    );
    assert_eq!(
        record["patch"][2]["insert"][0]["config"]["serverName"],
        "docs"
    );
    assert!(!Path::new(record["patchPath"].as_str().unwrap()).exists());
    assert!(!Path::new(record["settingsPath"].as_str().unwrap()).exists());
    assert_eq!(
        std::fs::read(fixture.root.join("external-settings.json")).unwrap(),
        original
    );

    for (id, name) in [("another-entry", "docs"), ("runtime-mcp-docs", "other")] {
        let fixture = dsh(false);
        let home = fixture.root.join("dsh-home");
        let dump_path = home.join("dump.json");
        let mut entries: Value =
            serde_json::from_slice(&std::fs::read(&dump_path).unwrap()).unwrap();
        entries.as_array_mut().unwrap().push(json!({
            "id": id,
            "name": "@deepseek-ai/dsh-mcp-client",
            "config": {"serverName": name, "transport": "streamable-http", "url": "http://127.0.0.1:43123/mcp/0123456789abcdef0123456789abcdef0123456789abcdef"}
        }));
        write(&dump_path, &entries.to_string());
        let output = turn_request(&fixture, mcp_request(None, "docs"));
        assert_eq!(output.status.code(), Some(2), "{id}/{name}");
        assert_eq!(output.stderr, b"configuration_metadata_invalid\n");
        assert!(!fixture.root.join("model-record.json").exists());
        assert_eq!(
            std::fs::read(fixture.root.join("external-settings.json")).unwrap(),
            original
        );
    }
}

#[test]
fn dsh缺少临时patch能力时在检查配置和提示词前拒绝mcp() {
    let fixture = dsh(false);
    fixture.script(
        "case \"$*\" in --version) printf '0.1.0-rc.6\\n';; --help) printf '%s\\n' '--profile <name>';; *) exit 41;; esac",
    );
    let output = turn_request(&fixture, mcp_request(None, "docs"));
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stderr, b"capabilities_unsupported\n");
    assert_eq!(
        std::fs::read_to_string(&fixture.calls).unwrap(),
        "--version\n--help\n"
    );
    assert!(!fixture.root.join("model-record.json").exists());
}
#[test]
fn dsh覆盖回合超时仍清理临时设置且保留用户原件() {
    let fixture = dsh(true);
    let mut child = command(&fixture, "turn")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"agentId":"fixture","prompt":"ask","model":"relay/model-v2"}"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("turn_timed_out")
    );
    let record: Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join("model-record.json")).unwrap())
            .unwrap();
    assert!(!Path::new(record["patchPath"].as_str().unwrap()).exists());
    assert!(fixture.root.join("external-settings.json").is_file());
}

#[test]
fn 危险环境引用与非字符串提供方不会变为默认配置() {
    for text in [
        "model_provider=42",
        "model_provider='relay'\n[model_providers.relay]\nenv_key='HOME'",
        "model='bad model'",
        "model_provider='relay'\n[model_providers.relay]\nenv_http_headers={ X=7 }",
    ] {
        let fixture = codex();
        write(&fixture.root.join("cli-home/config.toml"), text);
        assert_eq!(
            inspect(&fixture)["agents"][0]["error"],
            "configuration_metadata_invalid"
        );
        assert!(!fixture.calls.exists());
    }
}
#[test]
fn 未支持型号选择的适配器在真实执行前拒绝() {
    let fixture = codex();
    let raw = std::fs::read_to_string(&fixture.program)
        .unwrap()
        .replace("--image --model", "--image");
    std::fs::write(&fixture.program, raw).unwrap();
    let mut child = command(&fixture, "turn")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"agentId":"fixture","prompt":"ask","model":"relay/x"}"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("model_unsupported")
    );
    assert!(!fixture.root.join("arguments").exists());
    let mut config = fixture.document();
    config["agents"][0]["adapter"] = json!("native_acp");
    fixture.save(config);
    std::fs::remove_file(&fixture.calls).unwrap();
    let mut child = command(&fixture, "turn")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"agentId":"fixture","prompt":"ask","model":"relay/x"}"#)
        .unwrap();
    assert_eq!(child.wait_with_output().unwrap().status.code(), Some(2));
    assert!(!fixture.calls.exists());
}

#[test]
fn 元数据输出背压不能阻塞独立检查进程() {
    let fixture = codex();
    inspect(&fixture);
    let models = (0..511)
        .map(|i| json!({"slug":format!("model-{i:03}-{}","x".repeat(180))}))
        .collect::<Vec<_>>();
    write(
        &fixture.root.join("catalog.json"),
        &json!({"models":models}).to_string(),
    );
    let mut child = command(&fixture, "inspect")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while child.try_wait().unwrap().is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if child.try_wait().unwrap().is_none() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("metadata writer blocked");
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stderr, b"probe_timed_out\n");
}

#[test]
fn 配置合成超时按回合错误投影且不进入模型执行() {
    let fixture = dsh(false);
    let body = std::fs::read_to_string(&fixture.program)
        .unwrap()
        .replace("cat \"$DSH_HOME/dump.json\"", "while :; do :; done");
    std::fs::write(&fixture.program, body).unwrap();
    assert!(
        std::process::Command::new(&fixture.program)
            .arg("--fixture-warmup")
            .status()
            .unwrap()
            .success()
    );
    let mut child = command(&fixture, "turn")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"agentId":"fixture","prompt":"ask","model":"relay/model-v2"}"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stderr, b"turn_timed_out\n");
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("turn_timed_out")
    );
    assert!(!fixture.root.join("model-record.json").exists());
}

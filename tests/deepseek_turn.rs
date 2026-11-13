#![cfg(unix)]
mod support;
use iquji_shared_agent::{AgentError, Configuration, run_turn};
use serde_json::json;
use std::sync::atomic::AtomicBool;
use support::{
    codex::{cli, frames, invoke, request},
    deepseek::*,
    private_mode,
};
#[test]
fn deepseek_peer_fixture() {
    peer();
}

#[test]
fn 只读单回合返回无会话最终文本而不伪造事件() {
    let fixture = fixture("normal", 3000, 32768);
    let (result, events) = invoke(&fixture, None, &AtomicBool::new(false));
    let result = result.unwrap();
    assert!(result.session_id.is_none());
    assert!(!result.resumable);
    assert_eq!(result.text, "最终文本\n\t结束");
    assert!(events.is_empty());
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({"resumable":false,"text":"最终文本\n\t结束","stopReason":"end_turn"})
    );
    let record = record(&fixture);
    assert_eq!(record["mode"], "read-only");
    assert_eq!(record["dshMode"], 0o700);
    assert_eq!(record["homeMode"], 0o700);
    assert!(record["deepseekKey"].is_null());
    assert!(record["codexHome"].is_null());
    assert_cleaned(&fixture);
    assert_reaped(&fixture);
}

#[test]
fn 提示词单一位置参数拒绝选项解释与命令解释器注入() {
    let fixture = fixture("normal", 3000, 32768);
    let configuration = Configuration::load(&fixture.config).unwrap();
    let mut request = request(None);
    request.prompt = "--help ; $(printf unexpected)\n-V".into();
    run_turn(&configuration, request, &AtomicBool::new(false), |_| {
        panic!("DSH没有增量事件")
    })
    .unwrap();
    let bytes = std::fs::read(fixture.root.join("arguments")).unwrap();
    let args: Vec<_> = bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .collect();
    assert_eq!(
        args,
        vec![
            b"--profile".as_slice(),
            b"headless".as_slice(),
            " --help ; $(printf unexpected)\n-V".as_bytes()
        ]
    );
    assert_cleaned(&fixture);
}

#[test]
fn 任何恢复请求在版本帮助或执行启动前拒绝() {
    let fixture = fixture("normal", 3000, 32768);
    assert_eq!(
        invoke(
            &fixture,
            Some("actual-prior-thread"),
            &AtomicBool::new(false)
        )
        .0
        .unwrap_err(),
        AgentError::SessionUnsupported
    );
    assert!(!fixture.calls.exists());
    assert!(!fixture.root.join("metadata").exists());
}

#[test]
fn 非零退出缺凭据畸形与空回复均不产生成功文本() {
    for (scenario, error) in [
        ("failed", AgentError::AgentRequestFailed),
        ("credential", AgentError::ProviderCredentialMissing),
        (
            "credential-whitespace",
            AgentError::ProviderCredentialMissing,
        ),
        ("unknown-first", AgentError::AgentRequestFailed),
        ("invalid", AgentError::ProtocolInvalid),
        ("empty", AgentError::AgentRequestFailed),
    ] {
        let fixture = fixture(scenario, 3000, 32768);
        let (result, events) = invoke(&fixture, None, &AtomicBool::new(false));
        assert_eq!(result.unwrap_err(), error, "{scenario}");
        assert!(events.is_empty());
        assert_cleaned(&fixture);
        assert_reaped(&fixture);
        let output = cli(&fixture, &[]);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(output.stderr, format!("{}\n", error.code()).as_bytes());
        assert_eq!(
            frames(&output),
            vec![json!({"kind":"turn_error","error":error})]
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private-provider"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("partial-not-success"));
    }
}

#[test]
fn 默认环境隔离凭据与宿主只读覆盖且只产生终端帧() {
    let fixture = fixture("normal", 3000, 32768);
    let user = fixture.root.join("original-home");
    std::fs::create_dir(&user).unwrap();
    std::fs::write(user.join("settings.yaml"), "preserved").unwrap();
    let mut environment = support::CONFIGURATION_OVERRIDES
        .iter()
        .map(|name| (*name, user.as_os_str()))
        .collect::<Vec<_>>();
    environment.extend([
        ("HOME", user.as_os_str()),
        ("DSH_HOME", user.as_os_str()),
        (
            "DSH_PERMISSION_MODE",
            std::ffi::OsStr::new("workspace-write"),
        ),
        ("DEEPSEEK_API_KEY", std::ffi::OsStr::new("private-provider")),
    ]);
    let output = cli(&fixture, &environment);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        frames(&output),
        vec![
            json!({"kind":"turn_completed","resumable":false,"text":"最终文本\n\t结束","stopReason":"end_turn"})
        ]
    );
    let record = record(&fixture);
    assert_eq!(record["mode"], "read-only");
    assert!(record["deepseekKey"].is_null());
    assert_ne!(record["home"], user.to_str().unwrap());
    assert_ne!(record["dshHome"], user.to_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(user.join("settings.yaml")).unwrap(),
        "preserved"
    );
    assert_cleaned(&fixture);
}

#[test]
fn 显式配置目录与原生凭据引用不泄露给元数据也不改写用户文件() {
    for scenario in ["normal", "failed"] {
        let fixture = fixture(scenario, 3000, 32768);
        let user = fixture.root.join("selected");
        std::fs::create_dir(&user).unwrap();
        private_mode(&user, 0o750);
        std::fs::write(user.join("settings.yaml"), "preserved").unwrap();
        let mut config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&fixture.config).unwrap()).unwrap();
        config["agents"][0]["deepseekConfigurationDirectory"] = json!(user);
        config["agents"][0]["environmentVariables"] = json!(["FAKE_PROVIDER_KEY"]);
        fixture.save(config);
        let output = cli(
            &fixture,
            &[(
                "FAKE_PROVIDER_KEY",
                std::ffi::OsStr::new("private-provider"),
            )],
        );
        assert_eq!(output.status.success(), scenario == "normal");
        let record = record(&fixture);
        assert_eq!(record["dshHome"], user.to_str().unwrap());
        assert_eq!(record["dshMode"], 0o750);
        assert_eq!(record["provider"], "private-provider");
        assert_ne!(record["home"], user.to_str().unwrap());
        assert_eq!(
            std::fs::read_to_string(user.join("settings.yaml")).unwrap(),
            "preserved"
        );
        assert_cleaned(&fixture);
    }
}

#[test]
fn 元数据能力只报告真实单回合且配置不能覆盖只读策略() {
    let fixture = fixture("normal", 3000, 32768);
    let report = fixture.report();
    assert_eq!(report["connections"][0]["sessionMode"], "single_turn");
    assert_eq!(report["connections"][0]["supportedInputFormats"], json!([]));
    assert!(report["connections"][0]["protocolVersion"].is_null());
    assert_eq!(
        std::fs::read_to_string(&fixture.calls).unwrap(),
        "--version\n--help\n"
    );
    assert!(!fixture.root.join("record").exists());
    assert_cleaned(&fixture);
    for name in ["DSH_PERMISSION_MODE", "DSH_HOME", "NODE_OPTIONS"] {
        let mut config = fixture.document();
        config["agents"][0]["adapter"] = json!("deepseek_harness");
        config["agents"][0]["environmentVariables"] = json!([name]);
        fixture.save(config);
        assert_eq!(
            Configuration::load(&fixture.config).unwrap_err(),
            AgentError::ConfigurationInvalid
        );
    }
    let mut config = fixture.document();
    config["agents"][0]["deepseekConfigurationDirectory"] = json!(fixture.workspace);
    fixture.save(config);
    assert_eq!(
        Configuration::load(&fixture.config).unwrap_err(),
        AgentError::ConfigurationInvalid
    );
}

#[test]
fn 显式源码授权只启用工作目录写入预设() {
    let fixture = fixture("normal", 3000, 32768);
    let configuration = Configuration::load(&fixture.config).unwrap();
    let mut input = request(None);
    input.workspace_write = true;
    run_turn(&configuration, input, &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(record(&fixture)["mode"], "workspace-write");
    assert_cleaned(&fixture);
}

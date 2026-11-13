#![cfg(unix)]
mod support;
use iquji_shared_agent::{AgentError, Configuration};
use serde_json::json;
use std::sync::atomic::AtomicBool;
use support::{codex::*, private_mode};

#[test]
fn codex_peer_fixture() {
    peer();
}

#[test]
fn 固定只读参数与文本转译保持真实线程且不投影工具或推理() {
    let fixture = fixture("normal", 3000, 32768);
    let (result, events) = invoke(&fixture, None, &AtomicBool::new(false));
    let result = result.unwrap();
    assert_eq!(result.session_id.as_deref(), Some("thread-fixture"));
    assert!(result.resumable);
    assert_eq!(result.text, "正式回答\n\t结束");
    assert_eq!(result.stop_reason, "end_turn");
    assert_eq!(
        events,
        vec![
            json!({"kind":"session_started","sessionId":"thread-fixture","resumable":true}),
            json!({"kind":"text_delta","text":"正式回答\n\t结束"})
        ]
    );
    let arguments = std::fs::read(fixture.root.join("arguments")).unwrap();
    let arguments: Vec<_> = arguments
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .map(|value| String::from_utf8(value.to_vec()).unwrap())
        .collect();
    assert_eq!(
        arguments,
        vec![
            "exec",
            "--json",
            "--color",
            "never",
            "--sandbox",
            "read-only",
            "--skip-git-repo-check",
            "-C",
            fixture.workspace.to_str().unwrap(),
            "-"
        ]
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("record.prompt")).unwrap(),
        request(None).prompt
    );
    let record = record(&fixture);
    assert_eq!(record["codexMode"], 0o700);
    assert_eq!(record["homeMode"], 0o700);
    assert!(record["provider"].is_null());
    assert!(record["openai"].is_null());
    assert_cleaned(&fixture);
    assert_reaped(&fixture);
}

#[test]
fn codex_mcp_逐回合配置转发且不修改用户配置() {
    let fixture = fixture("normal", 3000, 32768);
    let selected = fixture.root.join("selected-config");
    std::fs::create_dir(&selected).unwrap();
    private_mode(&selected, 0o700);
    std::fs::write(selected.join("auth.json"), "private-auth-state").unwrap();
    private_mode(&selected.join("auth.json"), 0o600);
    std::fs::create_dir(selected.join("sessions")).unwrap();
    private_mode(&selected.join("sessions"), 0o700);
    let original = b"model = \"gpt-5\"\n[mcp_servers.keep]\nurl = \"https://example.invalid\"\n";
    std::fs::write(selected.join("config.toml"), original).unwrap();
    private_mode(&selected.join("config.toml"), 0o600);
    let mut config = fixture.document();
    config["agents"][0]["codexConfigurationDirectory"] = json!(selected);
    fixture.save(config);
    let mut request = request(None);
    request.mcp_servers = vec![iquji_agent_wire::McpServer {
        name: "studio_search".into(),
        url: format!("http://127.0.0.1:43123/mcp/{}", "a".repeat(48)),
    }];
    assert!(
        invoke_request(&fixture, request, &AtomicBool::new(false))
            .0
            .is_ok()
    );
    let arguments = std::fs::read(fixture.root.join("arguments")).unwrap();
    let arguments: Vec<_> = arguments
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .map(|value| String::from_utf8(value.to_vec()).unwrap())
        .collect();
    assert_eq!(arguments[0], "exec");
    assert!(!arguments.iter().any(|argument| argument.contains("/mcp/")));
    let record = record(&fixture);
    assert!(
        record["codexConfig"]
            .as_str()
            .unwrap()
            .contains("studio_search")
    );
    assert!(
        record["codexConfig"]
            .as_str()
            .unwrap()
            .contains(&"a".repeat(48))
    );
    assert!(record["authIsSymlink"].as_bool().unwrap());
    assert!(record["sessionsIsSymlink"].as_bool().unwrap());
    assert!(!std::path::Path::new(record["codexHome"].as_str().unwrap()).exists());
    assert_eq!(
        std::fs::read(selected.join("config.toml")).unwrap(),
        original
    );
    assert_cleaned(&fixture);
}

#[test]
fn codex_mcp_与显式或工作区配置冲突时在执行前拒绝() {
    for (location, filename) in [
        ("selected-config", "config.toml"),
        ("workspace/.codex", "config.toml"),
    ] {
        let fixture = fixture("normal", 3000, 32768);
        let directory = fixture.root.join(location);
        std::fs::create_dir_all(&directory).unwrap();
        private_mode(&directory, 0o700);
        std::fs::write(
            directory.join(filename),
            "[mcp_servers.studio_search]\nurl = \"http://127.0.0.1:9999\"\n",
        )
        .unwrap();
        private_mode(&directory.join(filename), 0o600);
        if location == "selected-config" {
            let mut config = fixture.document();
            config["agents"][0]["codexConfigurationDirectory"] = json!(directory);
            fixture.save(config);
        }
        let mut request = request(None);
        request.mcp_servers = vec![iquji_agent_wire::McpServer {
            name: "studio_search".into(),
            url: format!("http://127.0.0.1:43123/mcp/{}", "b".repeat(48)),
        }];
        assert_eq!(
            invoke_request(&fixture, request, &AtomicBool::new(false))
                .0
                .unwrap_err(),
            AgentError::ConfigurationMetadataInvalid,
            "{location}"
        );
        assert!(!fixture.root.join("arguments").exists());
        assert_cleaned(&fixture);
    }
}

#[test]
fn 续聊只传真实线程且不匹配或选项形态身份不能成功() {
    let fixture = fixture("normal", 3000, 32768);
    assert_eq!(
        invoke(&fixture, Some("thread-fixture"), &AtomicBool::new(false))
            .0
            .unwrap()
            .session_id
            .as_deref(),
        Some("thread-fixture")
    );
    let argv = std::fs::read(fixture.root.join("arguments")).unwrap();
    assert!(argv.ends_with(b"resume\0thread-fixture\0-\0"));
    let wrong = fixture_for("mismatch");
    assert_eq!(
        invoke(&wrong, Some("thread-fixture"), &AtomicBool::new(false))
            .0
            .unwrap_err(),
        AgentError::SessionMismatch
    );
    for id in ["-model", "../thread", "thread space", "thread;exec"] {
        let fixture = fixture_for("normal");
        assert_eq!(
            invoke(&fixture, Some(id), &AtomicBool::new(false))
                .0
                .unwrap_err(),
            AgentError::RequestInvalid
        );
        assert!(!fixture.calls.exists());
    }
}
fn fixture_for(scenario: &str) -> support::Fixture {
    fixture(scenario, 3000, 32768)
}

#[test]
fn 线程启动后的非终态诊断不投影且仍要求完整回答() {
    let fixture = fixture_for("startup-diagnostics");
    let (result, events) = invoke(&fixture, None, &AtomicBool::new(false));
    assert_eq!(result.unwrap().text, "正式回答\n\t结束");
    assert_eq!(
        events,
        vec![
            json!({"kind":"session_started","sessionId":"thread-fixture","resumable":true}),
            json!({"kind":"text_delta","text":"正式回答\n\t结束"})
        ]
    );
    assert_cleaned(&fixture);
    assert_reaped(&fixture);
}

#[test]
fn 零退出错误及不完整事件不得冒充回答() {
    for (scenario, error) in [
        ("credential", AgentError::ProviderCredentialMissing),
        ("failed", AgentError::AgentRequestFailed),
        ("malformed", AgentError::ProtocolInvalid),
        ("order", AgentError::ProtocolInvalid),
        ("startup-assistant", AgentError::ProtocolInvalid),
        ("duplicate", AgentError::ProtocolInvalid),
        ("partial", AgentError::AgentRequestFailed),
        ("no-assistant", AgentError::ProtocolInvalid),
    ] {
        let fixture = fixture_for(scenario);
        let (result, events) = invoke(&fixture, None, &AtomicBool::new(false));
        assert_eq!(result.unwrap_err(), error, "{scenario}");
        assert!(!events.iter().any(|event| event["kind"] == "text_delta"));
        assert_cleaned(&fixture);
        assert_reaped(&fixture);
    }
}

#[test]
fn 默认环境隔离用户配置凭据代理且命令行失败只返回稳定错误() {
    for scenario in ["normal", "credential"] {
        let fixture = fixture_for(scenario);
        let user = fixture.root.join("existing-user-home");
        std::fs::create_dir(&user).unwrap();
        std::fs::write(user.join("must-remain"), "sensitive-private").unwrap();
        let mut environment = support::CONFIGURATION_OVERRIDES
            .iter()
            .map(|name| (*name, user.as_os_str()))
            .collect::<Vec<_>>();
        environment.extend([
            ("HOME", user.as_os_str()),
            ("CODEX_HOME", user.as_os_str()),
            ("OPENAI_API_KEY", std::ffi::OsStr::new("sensitive-private")),
            ("HTTP_PROXY", std::ffi::OsStr::new("sensitive-private")),
        ]);
        let output = cli(&fixture, &environment);
        assert_eq!(
            output.status.code(),
            Some(if scenario == "normal" { 0 } else { 2 })
        );
        let frames = frames(&output);
        assert_eq!(
            frames.last().unwrap()["kind"],
            if scenario == "normal" {
                "turn_completed"
            } else {
                "turn_error"
            }
        );
        if scenario == "credential" {
            assert_eq!(output.stderr, b"provider_credential_missing\n");
        }
        assert!(!String::from_utf8_lossy(&output.stdout).contains("sensitive-private"));
        let record = record(&fixture);
        assert!(record["openai"].is_null());
        assert!(record["proxy"].is_null());
        assert_ne!(record["home"], user.to_str().unwrap());
        assert_ne!(record["codexHome"], user.to_str().unwrap());
        assert_eq!(
            std::fs::read_to_string(user.join("must-remain")).unwrap(),
            "sensitive-private"
        );
        assert_cleaned(&fixture);
    }
}

#[test]
fn 显式配置目录与环境引用只供回合且不改写用户目录() {
    let fixture = fixture_for("normal");
    let user = fixture.root.join("selected-config");
    std::fs::create_dir(&user).unwrap();
    private_mode(&user, 0o750);
    std::fs::write(user.join("provider-config"), "preserved").unwrap();
    let mut config = fixture.document();
    config["agents"][0]["codexConfigurationDirectory"] = json!(user);
    config["agents"][0]["environmentVariables"] = json!(["FAKE_PROVIDER_KEY"]);
    fixture.save(config);
    let output = cli(
        &fixture,
        &[
            (
                "FAKE_PROVIDER_KEY",
                std::ffi::OsStr::new("sensitive-private"),
            ),
            ("HOME", user.as_os_str()),
            ("OPENAI_API_KEY", std::ffi::OsStr::new("not-selected")),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record = record(&fixture);
    assert_eq!(record["codexHome"], user.to_str().unwrap());
    assert_ne!(record["home"], user.to_str().unwrap());
    assert_eq!(record["provider"], "sensitive-private");
    assert!(record["openai"].is_null());
    assert_eq!(record["codexMode"], 0o750);
    assert_eq!(
        std::fs::read_to_string(user.join("provider-config")).unwrap(),
        "preserved"
    );
    assert!(user.exists());
    assert_cleaned(&fixture);
}

#[test]
fn 危险环境引用与跨适配配置在启动前拒绝() {
    let fixture = fixture_for("normal");
    for name in [
        "LD_PRELOAD",
        "DYLD_INSERT_LIBRARIES",
        "NODE_OPTIONS",
        "BASH_ENV",
        "PYTHONPATH",
        "CODEX_HOME",
        "DSH_PERMISSION_MODE",
        "GIT_CONFIG_SYSTEM",
        "BAD-NAME",
    ] {
        let mut config = fixture.document();
        config["agents"][0]["environmentVariables"] = json!([name]);
        fixture.save(config);
        assert_eq!(
            Configuration::load(&fixture.config).unwrap_err(),
            AgentError::ConfigurationInvalid,
            "{name}"
        );
    }
    for values in [json!(["API_KEY", "API_KEY"]), json!(vec!["x"; 33])] {
        let mut config = fixture.document();
        config["agents"][0]["environmentVariables"] = values;
        fixture.save(config);
        assert_eq!(
            Configuration::load(&fixture.config).unwrap_err(),
            AgentError::ConfigurationInvalid
        );
    }
    let mut config = fixture.document();
    config["agents"][0]["adapter"] = json!("native_acp");
    config["agents"][0]["environmentVariables"] = json!(["API_KEY"]);
    fixture.save(config);
    assert_eq!(
        Configuration::load(&fixture.config).unwrap_err(),
        AgentError::ConfigurationInvalid
    );
    assert!(!fixture.calls.exists());
}

#[test]
fn 显式目录必须规范且过长引用值不会启动文本执行() {
    use std::os::unix::fs::symlink;
    let fixture = fixture_for("normal");
    let directory = fixture.root.join("selected");
    std::fs::create_dir(&directory).unwrap();
    let alias = fixture.root.join("alias");
    symlink(&directory, &alias).unwrap();
    for path in [alias, fixture.root.join("missing"), fixture.config.clone()] {
        let mut config = fixture.document();
        config["agents"][0]["codexConfigurationDirectory"] = json!(path);
        fixture.save(config);
        assert_eq!(
            Configuration::load(&fixture.config).unwrap_err(),
            AgentError::ConfigurationInvalid
        );
    }
    assert!(!fixture.calls.exists());
    let mut config = fixture.document();
    config["agents"][0]["environmentVariables"] = json!(["FAKE_PROVIDER_KEY"]);
    fixture.save(config);
    let value = "s".repeat(16_385);
    let output = cli(
        &fixture,
        &[("FAKE_PROVIDER_KEY", std::ffi::OsStr::new(&value))],
    );
    assert_eq!(output.stderr, b"configuration_invalid\n");
    assert!(!fixture.root.join("record").exists());
    assert_cleaned(&fixture);
}

#[test]
fn 配置覆盖变量即使显式引用也在任何元数据或执行前拒绝() {
    let fixture = support::Fixture::new();
    let ambient = fixture.root.join("ambient-config");
    std::fs::create_dir(&ambient).unwrap();
    std::fs::write(
        ambient.join("config.json"),
        "must-not-expose-private-config",
    )
    .unwrap();
    for adapter in ["codex_cli", "deepseek_harness"] {
        for name in [
            "HOME",
            "home",
            "CODEX_HOME",
            "DSH_HOME",
            "PATH",
            "LANG",
            "XDG_FUTURE_CONFIG",
            "xdg_config_home",
        ]
        .into_iter()
        .chain(support::CONFIGURATION_OVERRIDES.iter().copied())
        {
            let mut config = fixture.document();
            config["agents"][0]["adapter"] = json!(adapter);
            config["agents"][0]["environmentVariables"] = json!([name]);
            fixture.save(config);
            let output = cli(&fixture, &[(name, ambient.as_os_str())]);
            assert_eq!(output.status.code(), Some(2), "{adapter}/{name}");
            assert_eq!(output.stderr, b"configuration_invalid\n");
            assert_eq!(
                frames(&output),
                vec![json!({"kind":"turn_error","error":"configuration_invalid"})]
            );
            assert!(!fixture.calls.exists());
            assert!(
                !String::from_utf8_lossy(&output.stdout).contains("must-not-expose-private-config")
            );
        }
    }
    assert_eq!(
        std::fs::read_to_string(ambient.join("config.json")).unwrap(),
        "must-not-expose-private-config"
    );
}

#[test]
fn 显式源码授权使用工作目录写入沙箱() {
    let fixture = fixture("normal", 3000, 32768);
    let mut input = request(None);
    input.workspace_write = true;
    assert!(
        invoke_request(&fixture, input, &AtomicBool::new(false))
            .0
            .is_ok()
    );
    let arguments = std::fs::read(fixture.root.join("arguments")).unwrap();
    let values: Vec<_> = arguments.split(|byte| *byte == 0).collect();
    assert!(
        values
            .windows(2)
            .any(|pair| pair == [b"--sandbox".as_slice(), b"workspace-write".as_slice()])
    );
    assert!(values.windows(2).any(|pair| pair
        == [
            b"-C".as_slice(),
            fixture.workspace.to_str().unwrap().as_bytes()
        ]));
    assert_cleaned(&fixture);
}

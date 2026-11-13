#![cfg(unix)]
mod support;

use serde_json::{Value, json};
use support::{Fixture, private_mode, quote};

fn fixture(explicit: bool) -> Fixture {
    let fixture = Fixture::new();
    let home = fixture.root.join("dsh-home");
    std::fs::create_dir(&home).unwrap();
    private_mode(&home, 0o700);
    let mut settings = json!({"id":"settings","name":"@deepseek-ai/dsh-settings-file"});
    if explicit {
        settings["config"] = json!({"path":home.join("settings.yaml")});
    }
    let entries = json!([
        {"id":"model","name":"@deepseek-ai/dsh-agent-default-model","config":{"provider":"relay","model":"relay/default"}},
        settings,
        {"id":"provider","name":"@deepseek-ai/dsh-llm-pi-ai","config":{"providers":{"relay":{"apiKeyEnv":"FAKE_DSH_KEY"}}}}
    ]);
    let dump = fixture.root.join("dump.json");
    std::fs::write(&dump, entries.to_string()).unwrap();
    private_mode(&dump, 0o600);
    let mut config = fixture.document();
    config["agents"][0]["adapter"] = json!("deepseek_harness");
    config["agents"][0]["deepseekConfigurationDirectory"] = json!(home);
    config["timeoutMs"] = json!(1000);
    fixture.save(config);
    fixture.script(&format!(
        "case \"$*\" in --version) printf '0.1.0-rc.6\\n';; --help) printf '%s\\n' '--profile --patch --dump-config';; '--profile headless --dump-config') cat {};; *) exit 44;; esac",
        quote(&dump)
    ));
    fixture
}

fn inspect(fixture: &Fixture) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_iquji-shared-agent"))
        .args([
            "--config",
            fixture.config.to_str().unwrap(),
            "inspect",
            "fixture",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(&fixture.root)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains(fixture.root.to_str().unwrap()));
    assert!(!text.contains("private-import-marker"));
    serde_json::from_str(&text).unwrap()
}

#[test]
fn 缺失默认设置文档沿用正式组合配置且不读取迁移备份() {
    let fixture = fixture(false);
    std::fs::write(
        fixture.root.join("dsh-home/settings.yaml.imported"),
        "private-import-marker: [invalid",
    )
    .unwrap();
    let report = inspect(&fixture);
    let metadata = &report["agents"][0]["metadata"];
    assert_eq!(metadata["provider"], "relay");
    assert_eq!(metadata["configuredModel"], "relay/default");
    assert_eq!(
        metadata["credentialEnvironmentVariables"],
        json!(["FAKE_DSH_KEY"])
    );
    assert!(!fixture.root.join("dsh-home/settings.yaml").exists());
}

#[test]
fn 默认设置不安全或显式设置缺失继续拒绝() {
    use std::os::unix::fs::symlink;
    for case in ["symlink", "writable", "invalid", "explicit-missing"] {
        let fixture = fixture(case == "explicit-missing");
        let path = fixture.root.join("dsh-home/settings.yaml");
        match case {
            "symlink" => symlink(fixture.root.join("absent.yaml"), &path).unwrap(),
            "writable" => {
                std::fs::write(&path, "{}").unwrap();
                private_mode(&path, 0o666);
            }
            "invalid" => {
                std::fs::write(&path, "agent-default-model: [invalid").unwrap();
                private_mode(&path, 0o600);
            }
            "explicit-missing" => {}
            _ => unreachable!(),
        }
        let report = inspect(&fixture);
        assert!(report["agents"][0]["metadata"].is_null(), "{case}");
        assert_eq!(
            report["agents"][0]["error"],
            if case == "invalid" {
                "configuration_metadata_invalid"
            } else {
                "configuration_metadata_unavailable"
            },
            "{case}"
        );
    }
}

#[test]
fn 官方模型元数据使用当前_api_key_组件配置() {
    let fixture = fixture(false);
    let entries = json!([
        {"id":"agent-default-model","name":"@deepseek-ai/dsh-agent-default-model","config":{"provider":"deepseek-official","model":"deepseek-flash"}},
        {"id":"llm-deepseek","name":"@deepseek-ai/dsh-llm-deepseek-api-key","config":{"apiKeyEnv":"FAKE_DEEPSEEK_KEY","models":[{"id":"deepseek-flash"},{"id":"deepseek-pro"}]}}
    ]);
    std::fs::write(fixture.root.join("dump.json"), entries.to_string()).unwrap();
    let report = inspect(&fixture);
    let metadata = &report["agents"][0]["metadata"];
    assert_eq!(metadata["provider"], "deepseek-official");
    assert_eq!(metadata["configuredModel"], "deepseek-flash");
    assert_eq!(
        metadata["supportedModels"],
        json!(["deepseek-flash", "deepseek-pro"])
    );
    assert_eq!(
        metadata["credentialEnvironmentVariables"],
        json!(["FAKE_DEEPSEEK_KEY"])
    );
}

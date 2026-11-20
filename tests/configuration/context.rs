use super::*;

#[test]
fn 检查上下文绑定实际制品与配置目录而不混同配置内容指纹() {
    let fixture = codex();
    let before = inspect(&fixture);
    let program = std::fs::read_to_string(&fixture.program).unwrap();
    std::fs::write(&fixture.program, format!("{program}\n# fixture change\n")).unwrap();
    let changed_program = inspect(&fixture);
    assert_ne!(
        before["agents"][0]["inspectionContextFingerprint"],
        changed_program["agents"][0]["inspectionContextFingerprint"]
    );
    assert_eq!(
        before["agents"][0]["metadata"],
        changed_program["agents"][0]["metadata"]
    );
    let other = directory(&fixture, "other-cli-home");
    let source = std::fs::read_to_string(fixture.root.join("cli-home/config.toml")).unwrap();
    write(&other.join("config.toml"), &source);
    let mut config = fixture.document();
    config["agents"][0]["codexConfigurationDirectory"] = json!(other);
    fixture.save(config);
    let changed_directory = inspect(&fixture);
    assert_ne!(
        changed_program["agents"][0]["inspectionContextFingerprint"],
        changed_directory["agents"][0]["inspectionContextFingerprint"]
    );
    assert_eq!(
        changed_program["agents"][0]["metadata"],
        changed_directory["agents"][0]["metadata"]
    );
    assert!(!fixture.calls.exists());
}

#[test]
fn 检查在读取型号前拒绝不可用或超限的实际制品() {
    for case in ["missing", "directory", "not-executable", "digest", "large"] {
        let fixture = codex();
        match case {
            "missing" => std::fs::remove_file(&fixture.program).unwrap(),
            "directory" => {
                std::fs::remove_file(&fixture.program).unwrap();
                std::fs::create_dir(&fixture.program).unwrap();
            }
            "not-executable" => private_mode(&fixture.program, 0o600),
            "digest" => {
                let mut config: Value =
                    serde_json::from_slice(&std::fs::read(&fixture.config).unwrap()).unwrap();
                config["agents"][0]["expectedSha256"] = json!("a".repeat(64));
                fixture.save(config);
            }
            "large" => std::fs::OpenOptions::new()
                .write(true)
                .open(&fixture.program)
                .unwrap()
                .set_len(512 * 1024 * 1024 + 1)
                .unwrap(),
            _ => unreachable!(),
        }
        let report = inspect(&fixture);
        let expected = match case {
            "directory" => "executable_not_regular",
            "not-executable" => "executable_not_executable",
            "digest" => "executable_digest_mismatch",
            _ => "executable_unavailable",
        };
        assert_eq!(report["agents"][0]["error"], expected, "{case}");
        assert!(report["agents"][0]["metadata"].is_null(), "{case}");
        assert!(
            report["agents"][0]["inspectionContextFingerprint"].is_null(),
            "{case}"
        );
    }
}

#[test]
fn 检查取消不读取元数据且缺显式dsh目录不产生随机上下文() {
    use iquji_shared_agent::{Configuration, inspect_configuration};
    use std::sync::atomic::AtomicBool;
    let fixture = codex();
    let configuration = Configuration::load(&fixture.config).unwrap();
    let report = inspect_configuration(&configuration, &AtomicBool::new(true));
    assert_eq!(
        report.agents[0].error,
        Some(iquji_agent_wire::AgentError::ProbeCancelled)
    );
    assert!(report.agents[0].metadata.is_none());
    assert!(report.agents[0].inspection_context_fingerprint.is_none());
    assert!(!fixture.calls.exists());

    let fixture = dsh(false);
    let mut document: Value =
        serde_json::from_slice(&std::fs::read(&fixture.config).unwrap()).unwrap();
    document["agents"][0]
        .as_object_mut()
        .unwrap()
        .remove("deepseekConfigurationDirectory");
    fixture.save(document);
    let report = inspect(&fixture);
    assert_eq!(
        report["agents"][0]["error"],
        "configuration_metadata_unavailable"
    );
    assert!(report["agents"][0]["inspectionContextFingerprint"].is_null());
    assert!(!fixture.calls.exists());
}

#[test]
fn 型号检查期间制品改变时不能返回成功身份() {
    let fixture = dsh(false);
    let body = std::fs::read_to_string(&fixture.program).unwrap().replace(
        "cat \"$DSH_HOME/dump.json\"",
        "printf '\\n# fixture changed\\n' >> \"$0\"; cat \"$DSH_HOME/dump.json\"",
    );
    std::fs::write(&fixture.program, body).unwrap();
    let report = inspect(&fixture);
    assert_eq!(report["agents"][0]["error"], "executable_digest_mismatch");
    assert!(report["agents"][0]["metadata"].is_null());
    assert!(report["agents"][0]["inspectionContextFingerprint"].is_null());
}

#[test]
fn 单agent检查不启动其他配置项且通用检查仍遍历全部() {
    let fixture = codex();
    let dsh_home = directory(&fixture, "unused-dsh-home");
    let dsh_program = fixture.root.join("unused-dsh");
    let invoked = fixture.root.join("unused-dsh-called");
    std::fs::write(
        &dsh_program,
        format!("#!/bin/sh\nif [ \"$1\" = --fixture-warmup ]; then exit 0; fi\nprintf invoked > {}\nexit 42\n", quote(&invoked)),
    )
    .unwrap();
    private_mode(&dsh_program, 0o700);
    assert!(
        std::process::Command::new(&dsh_program)
            .arg("--fixture-warmup")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .status()
            .unwrap()
            .success()
    );
    let mut document: Value =
        serde_json::from_slice(&std::fs::read(&fixture.config).unwrap()).unwrap();
    document["agents"].as_array_mut().unwrap().push(json!({
        "id":"unused-dsh", "adapter":"deepseek_harness", "executable":dsh_program,
        "deepseekConfigurationDirectory":dsh_home
    }));
    fixture.save(document);
    let output = command(&fixture, "inspect")
        .arg("fixture")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let report: iquji_agent_wire::ConfigurationInspection =
        serde_json::from_slice(&output.stdout).unwrap();
    report.validate().unwrap();
    assert_eq!(report.agents.len(), 1);
    assert_eq!(report.agents[0].id, "fixture");
    assert!(report.agents[0].metadata.is_some());
    assert!(!invoked.exists());
    assert!(!fixture.calls.exists());
    let report = inspect(&fixture);
    assert_eq!(report["agents"].as_array().unwrap().len(), 2);
    assert!(report["agents"][1]["metadata"].is_null());
    assert!(invoked.exists());
}

#[test]
fn 未知或非法agent选择保持稳定错误且不检查任何配置项() {
    use iquji_shared_agent::{AgentError, Configuration, inspect_configuration};
    use std::sync::atomic::AtomicBool;
    let fixture = codex();
    for id in ["unknown-agent", "../fixture", "--help", ""] {
        let output = command(&fixture, "inspect").arg(id).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{id}");
        assert!(output.stdout.is_empty(), "{id}");
        assert_eq!(output.stderr, b"request_invalid\n", "{id}");
        let mut configuration = Configuration::load(&fixture.config).unwrap();
        assert_eq!(
            configuration.select_agent(id),
            Err(AgentError::RequestInvalid)
        );
        assert_eq!(
            inspect_configuration(&configuration, &AtomicBool::new(false))
                .agents
                .len(),
            1
        );
    }
    assert!(!fixture.calls.exists());
}

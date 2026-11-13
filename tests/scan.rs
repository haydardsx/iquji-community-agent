#![cfg(unix)]
mod support;

use serde_json::json;
use sha2::{Digest, Sha256};
use support::{CODEX, Fixture, private_mode, quote};

#[test]
fn 固定参数扫描不执行登录或模型回合并如实保留能力边界() {
    let fixture = Fixture::new();
    let report = fixture.report();
    assert_eq!(report["schemaVersion"], 1);
    assert_eq!(report["kind"], "agent_connection_scan");
    assert_eq!(report["readiness"], "not_checked");
    let connection = &report["connections"][0];
    assert_eq!(connection["status"], "detected");
    assert_eq!(connection["version"], "0.50.0");
    assert_eq!(connection["sessionMode"], "resumable");
    assert_eq!(
        connection["supportedInputFormats"],
        json!(["image/png", "image/jpeg"])
    );
    assert_eq!(
        std::fs::read_to_string(&fixture.calls).unwrap(),
        "--version\nexec --help\nexec resume --help\n"
    );
    let output = report.to_string();
    assert!(!output.contains(fixture.root.to_str().unwrap()));
    assert!(!output.contains("executable"));
    assert!(!output.contains("workingDirectory"));
}

#[test]
fn 帮助中以逗号分隔的短选项仍能被识别() {
    let fixture = Fixture::new();
    fixture.script(&CODEX.replace("-C resume", "-C, --cd <DIR>\nresume"));
    assert_eq!(fixture.report()["connections"][0]["status"], "detected");
    assert_eq!(
        std::fs::read_to_string(&fixture.calls).unwrap(),
        "--version\nexec --help\nexec resume --help\n"
    );
}

#[test]
fn 图片必须由首轮与续聊两个入口都声明() {
    let fixture = Fixture::new();
    fixture.script(&CODEX.replace("'--json --image'", "'--json'"));
    let connection = &fixture.report()["connections"][0];
    assert_eq!(connection["status"], "detected");
    assert_eq!(connection["supportedInputFormats"], json!([]));
}

#[test]
fn 单回合适配不伪造线程续聊或图片能力() {
    let fixture = Fixture::new();
    fixture.script("case \"$*\" in --version) echo '0.1.0-rc.6';; --help) echo '--profile <name>';; *) exit 42;; esac");
    let mut config = fixture.document();
    config["agents"][0]["adapter"] = json!("deepseek_harness");
    fixture.save(config);
    let connection = &fixture.report()["connections"][0];
    assert_eq!(connection["status"], "detected");
    assert_eq!(connection["version"], "0.1.0");
    assert_eq!(connection["sessionMode"], "single_turn");
    assert_eq!(connection["supportedInputFormats"], json!([]));
    assert_eq!(
        std::fs::read_to_string(&fixture.calls).unwrap(),
        "--version\n--help\n"
    );
}

#[test]
fn 低版本畸形横幅缺少参数和失败进程均返回稳定错误() {
    let fixture = Fixture::new();
    for banner in ["codex-cli 0.19.9", "v22", "1.2.3.4", "invalid-version"] {
        fixture.script(&CODEX.replace("codex-cli 0.50.0", banner));
        fixture.assert_error("version_unsupported");
    }
    fixture.script(&CODEX.replace("--sandbox", "--sandbox-extra"));
    fixture.assert_error("capabilities_unsupported");
    fixture.script("printf 'private-stderr-secret /private/local/path' >&2; exit 7");
    let report = fixture.report();
    assert_eq!(report["connections"][0]["error"], "process_failed");
    assert!(!report.to_string().contains("private-stderr-secret"));
}

#[test]
fn 实际文件摘要不符或缺少执行权限时不会启动进程() {
    let fixture = Fixture::new();
    let mut config = fixture.document();
    config["agents"][0]["expectedSha256"] = json!("0".repeat(64));
    fixture.save(config.clone());
    fixture.assert_error("executable_digest_mismatch");
    assert!(!fixture.calls.exists());
    config["agents"][0]["expectedSha256"] = json!(format!(
        "{:x}",
        Sha256::digest(std::fs::read(&fixture.program).unwrap())
    ));
    fixture.save(config);
    assert_eq!(fixture.report()["connections"][0]["status"], "detected");
    std::fs::remove_file(&fixture.calls).unwrap();
    private_mode(&fixture.program, 0o600);
    fixture.assert_error("executable_not_executable");
    assert!(!fixture.calls.exists());
}

#[test]
fn 显式可执行入口可用符号链接但目标必须是普通可执行文件() {
    let fixture = Fixture::new();
    let target = fixture.root.join("real-program");
    std::fs::rename(&fixture.program, &target).unwrap();
    std::os::unix::fs::symlink(&target, &fixture.program).unwrap();
    assert_eq!(fixture.report()["connections"][0]["status"], "detected");
    std::fs::remove_file(&fixture.program).unwrap();
    std::os::unix::fs::symlink(&fixture.workspace, &fixture.program).unwrap();
    fixture.assert_error("executable_not_regular");
}

#[test]
fn 清空用户环境且固定工作目录不会读取或改写宿主配置() {
    let fixture = Fixture::new();
    let home = fixture.root.join("private-home");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(home.join("do-not-read"), "preserved-provider-model").unwrap();
    let observed = fixture.root.join("observed-cwd");
    fixture.script(&format!(r#"
if [ "${{OPENAI_API_KEY+x}}${{AWS_SECRET_ACCESS_KEY+x}}${{HTTP_PROXY+x}}${{NODE_OPTIONS+x}}" != "" ]; then exit 41; fi
if [ "$HOME" != "$CODEX_HOME" ] || [ ! -d "$HOME" ] || [ -f "$HOME/do-not-read" ]; then exit 41; fi
printf '%s' "$PWD" > {}
{CODEX}
"#, quote(&observed)));
    let output = fixture
        .command()
        .env("HOME", &home)
        .env("CODEX_HOME", &home)
        .env("OPENAI_API_KEY", "credential-stays-local")
        .env("AWS_SECRET_ACCESS_KEY", "platform-secret")
        .env("HTTP_PROXY", "http://example.invalid")
        .env("NODE_OPTIONS", "--invalid")
        .env("PATH", ".::relative:/usr/bin:/bin")
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["connections"][0]["status"], "detected");
    assert_eq!(
        std::fs::read_to_string(observed).unwrap(),
        fixture.workspace.to_str().unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(home.join("do-not-read")).unwrap(),
        "preserved-provider-model"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("credential-stays-local"));
}

#[test]
fn 未实现适配未知字段和请求命令注入均在进程启动前拒绝() {
    let fixture = Fixture::new();
    let mut cases = Vec::new();
    for adapter in ["claude_cli", "grok_cli", "acp", "mcp"] {
        let mut value = fixture.document();
        value["agents"][0]["adapter"] = json!(adapter);
        cases.push(value);
    }
    let mut value = fixture.document();
    value["agents"][0]["arguments"] = json!(["-c", "touch injected"]);
    cases.push(value);
    let mut value = fixture.document();
    value["agents"][0]["environment"] = json!({"KEY":"secret"});
    cases.push(value);
    let mut value = fixture.document();
    let duplicate = value["agents"][0].clone();
    value["agents"].as_array_mut().unwrap().push(duplicate);
    cases.push(value);
    let mut value = fixture.document();
    value["agents"][0]["executable"] = json!("relative-program");
    cases.push(value);
    let mut value = fixture.document();
    value["timeoutMs"] = json!(0);
    cases.push(value);
    let mut value = fixture.document();
    value["maxOutputBytes"] = json!(1000000);
    cases.push(value);
    let mut value = fixture.document();
    value["schemaVersion"] = json!(2);
    cases.push(value);
    for value in cases {
        fixture.save(value);
        fixture.assert_configuration_error(
            fixture.command().output().unwrap(),
            "configuration_invalid",
        );
        assert!(!fixture.calls.exists());
    }
}

#[test]
fn 配置必须由当前用户私有持有且不能通过符号链接传入() {
    let fixture = Fixture::new();
    private_mode(&fixture.config, 0o644);
    fixture.assert_configuration_error(
        fixture.command().output().unwrap(),
        "configuration_not_private",
    );
    private_mode(&fixture.config, 0o600);
    let moved = fixture.root.join("config-real");
    std::fs::rename(&fixture.config, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &fixture.config).unwrap();
    fixture
        .assert_configuration_error(fixture.command().output().unwrap(), "configuration_invalid");
    assert!(!fixture.calls.exists());
}

#[test]
fn 入口不从当前位置或宿主变量猜测配置() {
    let fixture = Fixture::new();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_iquji-shared-agent"))
        .current_dir(&fixture.root)
        .env("SHARED_AGENT_CONFIG", &fixture.config)
        .output()
        .unwrap();
    fixture.assert_configuration_error(output, "configuration_invalid");
    assert!(!fixture.calls.exists());
}

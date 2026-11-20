use super::*;
use iquji_agent_wire::{ConfigurationInspection, PreparedConfigurationFile};

fn prepare(fixture: &Fixture, destination: &Path) -> Value {
    let output = command(fixture, "inspect")
        .args(["fixture", "--model", "relay/model-v2", "--output-directory"])
        .arg(destination)
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
    assert!(!text.contains("relay.invalid"));
    assert!(!text.contains(fixture.root.to_str().unwrap()));
    let report: ConfigurationInspection = serde_json::from_str(&text).unwrap();
    report.validate().unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn 准备私有覆盖保留有效provider与settings且不执行回合() {
    use sha2::{Digest, Sha256};
    let fixture = dsh(false);
    let destination = directory(&fixture, "host-artifacts");
    let original = std::fs::read(fixture.root.join("external-settings.json")).unwrap();
    let report = prepare(&fixture, &destination);
    assert!(report["agents"][0]["error"].is_null());
    let prepared = report["agents"][0]["preparedModelOverride"].clone();
    assert_eq!(prepared["model"], "relay/model-v2");
    for (name, key) in [("model.patch.json", "patch"), ("settings.json", "settings")] {
        let bytes = std::fs::read(destination.join(name)).unwrap();
        let descriptor: PreparedConfigurationFile =
            serde_json::from_value(prepared[key].clone()).unwrap();
        assert_eq!(descriptor.size, bytes.len() as u64);
        assert_eq!(descriptor.sha256, format!("{:x}", Sha256::digest(bytes)));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(destination.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let patch: Value =
        serde_json::from_slice(&std::fs::read(destination.join("model.patch.json")).unwrap())
            .unwrap();
    assert_eq!(
        patch[0]["config"],
        json!({"provider":"relay","model":"relay/model-v2"})
    );
    assert_eq!(
        patch[1]["config"]["path"],
        json!(destination.join("settings.json"))
    );
    let copied: Value =
        serde_json::from_slice(&std::fs::read(destination.join("settings.json")).unwrap()).unwrap();
    let source: Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(copied["llm-pi-ai"], source["llm-pi-ai"]);
    assert_eq!(copied["private-setting"], source["private-setting"]);
    assert_eq!(copied["agent-default-model"]["model"], "relay/model-v2");
    assert_eq!(
        std::fs::read(fixture.root.join("external-settings.json")).unwrap(),
        original
    );
    assert!(!fixture.root.join("model-record.json").exists());
}

#[test]
fn 不安全非空或符号链接输出目录在任何cli启动前拒绝() {
    use std::os::unix::fs::symlink;
    for kind in ["permission", "occupied", "symlink"] {
        let fixture = dsh(false);
        let destination = directory(&fixture, "host-artifacts");
        match kind {
            "permission" => private_mode(&destination, 0o750),
            "occupied" => write(&destination.join("keep"), "caller-owned"),
            "symlink" => {
                std::fs::remove_dir(&destination).unwrap();
                symlink(directory(&fixture, "outside"), &destination).unwrap();
            }
            _ => unreachable!(),
        }
        let report = prepare(&fixture, &destination);
        assert!(report["agents"][0]["metadata"].is_null());
        assert!(report["agents"][0].get("preparedModelOverride").is_none());
        assert!(!fixture.calls.exists());
        if kind == "occupied" {
            assert_eq!(
                std::fs::read_to_string(destination.join("keep")).unwrap(),
                "caller-owned"
            );
        }
    }
}

#[test]
fn dsh工作目录变化会改变检查上下文而不混入元数据() {
    let fixture = dsh(false);
    let before = inspect(&fixture);
    let workspace = directory(&fixture, "another-workspace");
    let mut document: Value =
        serde_json::from_slice(&std::fs::read(&fixture.config).unwrap()).unwrap();
    document["workingDirectory"] = json!(workspace);
    fixture.save(document);
    let after = inspect(&fixture);
    assert_eq!(
        before["agents"][0]["metadata"],
        after["agents"][0]["metadata"]
    );
    assert_ne!(
        before["agents"][0]["inspectionContextFingerprint"],
        after["agents"][0]["inspectionContextFingerprint"]
    );
}

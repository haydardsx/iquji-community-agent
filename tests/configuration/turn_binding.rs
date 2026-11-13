use super::*;
use iquji_agent_wire::{Adapter, turn_context_fingerprint};
use sha2::{Digest, Sha256};

fn bound_request(fixture: &Fixture) -> Value {
    json!({"agentId":"fixture","prompt":"ask","model":"relay/model-v2",
        "contextFingerprint":turn_context_fingerprint(Adapter::DeepseekHarness,
            &format!("{:x}",Sha256::digest(std::fs::read(&fixture.program).unwrap())),
            fixture.workspace.to_str().unwrap(),Some(fixture.root.join("dsh-home").to_str().unwrap())).unwrap()})
}

fn invoke(fixture: &Fixture, request: &Value) -> std::process::Output {
    let mut child = command(fixture, "turn")
        .env("FAKE_DSH_KEY", "fixture-key")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(request).unwrap())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn 受绑定回合只在实际制品配置根及工作区一致时执行() {
    for changed in ["none", "executable", "workspace", "configuration"] {
        let fixture = dsh(false);
        let request = bound_request(&fixture);
        match changed {
            "executable" => {
                let body = std::fs::read_to_string(&fixture.program).unwrap();
                std::fs::write(&fixture.program, format!("{body}\n# 已替换的假制品\n")).unwrap();
            }
            "workspace" | "configuration" => {
                let mut config: Value =
                    serde_json::from_slice(&std::fs::read(&fixture.config).unwrap()).unwrap();
                let other = directory(&fixture, "other");
                if changed == "workspace" {
                    config["workingDirectory"] = json!(other);
                } else {
                    config["agents"][0]["deepseekConfigurationDirectory"] = json!(other);
                }
                fixture.save(config);
            }
            _ => {}
        }
        let output = invoke(&fixture, &request);
        if changed == "none" {
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(fixture.root.join("model-record.json").exists());
        } else {
            assert_eq!(output.status.code(), Some(2), "{changed}");
            assert_eq!(output.stderr, b"executable_digest_mismatch\n");
            assert!(!fixture.calls.exists(), "{changed}");
        }
    }
}

pub(super) fn mutate_if_requested(patch: &Path, settings: &Path) {
    use std::os::unix::fs::symlink;
    let Ok(attack) = std::env::var("MODEL_ATTACK") else {
        return;
    };
    let root = patch.parent().unwrap();
    let record = std::path::PathBuf::from(std::env::var_os("MODEL_RECORD").unwrap());
    let outside = record.parent().unwrap().join("outside");
    match attack.as_str() {
        "content" => std::fs::write(settings, b"{}").unwrap(),
        "permission" => private_mode(patch, 0o640),
        "symlink" => {
            std::fs::remove_file(settings).unwrap();
            symlink(outside.join("keep"), settings).unwrap();
        }
        "directory" => {
            std::fs::rename(root, root.with_extension("moved")).unwrap();
            symlink(outside, root).unwrap();
        }
        _ => unreachable!(),
    }
}

#[test]
fn 实际cli改写覆盖或替换目录不能返回成功且清理不遍历外部目标() {
    for attack in ["content", "permission", "symlink", "directory"] {
        let fixture = dsh(false);
        let outside = directory(&fixture, "outside");
        write(&outside.join("keep"), "outside-content");
        let body = std::fs::read_to_string(&fixture.program).unwrap().replace(
            "MODEL_SLEEP=no",
            &format!("MODEL_ATTACK={attack} MODEL_SLEEP=no"),
        );
        std::fs::write(&fixture.program, body).unwrap();
        let output = invoke(&fixture, &bound_request(&fixture));
        assert_eq!(output.status.code(), Some(2), "{attack}");
        assert_eq!(
            output.stderr, b"configuration_metadata_invalid\n",
            "{attack}"
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("model reply"));
        assert_eq!(
            std::fs::read(outside.join("keep")).unwrap(),
            b"outside-content"
        );
        let record: Value =
            serde_json::from_slice(&std::fs::read(fixture.root.join("model-record.json")).unwrap())
                .unwrap();
        let patch = Path::new(record["patchPath"].as_str().unwrap());
        let root = patch.parent().unwrap();
        if attack == "directory" {
            assert!(!root.with_extension("moved").join("settings.json").exists());
            assert!(
                !root
                    .with_extension("moved")
                    .join("model.patch.json")
                    .exists()
            );
            std::fs::remove_file(root).unwrap();
            std::fs::remove_dir(root.with_extension("moved")).unwrap();
        } else {
            assert!(!root.exists());
        }
    }
}

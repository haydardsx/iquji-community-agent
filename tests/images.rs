#![cfg(unix)]
mod support;
use iquji_shared_agent::{AgentError, Attachment};
use std::sync::atomic::{AtomicBool, Ordering};
use support::{images::*, private_mode};
#[test]
fn image_peer_fixture() {
    peer();
}

#[test]
fn 只读命令首轮续聊均传隔离快照并在完成后删除暂存() {
    for resume in [false, true] {
        let fixture = fixture("normal", "codex_cli");
        let attachments = vec![image(&fixture, "image/png"), image(&fixture, "image/jpeg")];
        let (result, _) = invoke(&fixture, request(attachments, resume));
        assert_eq!(result.unwrap().text, "图片已收到");
        assert_staged_removed(&fixture);
        assert!(fixture.root.join("inputs/图片;literal.png").exists());
    }
}
#[test]
fn 正式协议原生与外部适配按能力发送图片字节且不授权读文件() {
    for adapter in ["native_acp", "external_acp_adapter"] {
        for resume in [false, true] {
            let fixture = fixture("normal", adapter);
            let attachments = vec![image(&fixture, "image/png"), image(&fixture, "image/jpeg")];
            let (result, _) = invoke(&fixture, request(attachments, resume));
            assert_eq!(result.unwrap().text, "图片已收到");
            let prompt = std::fs::read_to_string(fixture.root.join("prompt.json")).unwrap();
            assert!(!prompt.contains(fixture.root.to_str().unwrap()));
        }
    }
}
#[test]
fn 能力缺失不会读取图片且单回合适配在子进程前拒绝() {
    for (adapter, mode) in [
        ("codex_cli", "missing-first"),
        ("codex_cli", "missing-resume"),
        ("native_acp", "unsupported"),
        ("external_acp_adapter", "unsupported"),
        ("deepseek_harness", "normal"),
    ] {
        let fixture = fixture(mode, adapter);
        let mut attachment = image(&fixture, "image/png");
        let Attachment::Image { path, .. } = &mut attachment;
        std::fs::remove_file(&*path).unwrap();
        assert_eq!(
            invoke(&fixture, request(vec![attachment], false))
                .0
                .unwrap_err(),
            AgentError::AttachmentUnsupported,
            "{adapter}/{mode}"
        );
        assert!(!fixture.root.join("staged.json").exists());
        if adapter == "deepseek_harness" {
            assert!(!fixture.calls.exists());
        }
    }
}
#[test]
fn 权限摘要声明类型与规范路径不匹配不能进入执行() {
    for variant in [
        "writable",
        "directory-mode",
        "digest",
        "media",
        "size",
        "signature",
        "missing",
        "symlink",
        "alias",
        "fifo",
    ] {
        let fixture = fixture("normal", "codex_cli");
        let mut attachment = image(&fixture, "image/png");
        let Attachment::Image {
            path,
            media_type,
            size,
            sha256,
        } = &mut attachment;
        match variant {
            "writable" => private_mode(std::path::Path::new(path), 0o600),
            "directory-mode" => private_mode(std::path::Path::new(path).parent().unwrap(), 0o755),
            "digest" => *sha256 = "a".repeat(64),
            "media" => *media_type = "image/jpeg".into(),
            "size" => *size += 1,
            "signature" => {
                private_mode(std::path::Path::new(path), 0o600);
                let invalid = vec![0; *size as usize];
                use sha2::Digest as _;
                *sha256 = format!("{:x}", sha2::Sha256::digest(&invalid));
                std::fs::write(&*path, invalid).unwrap();
                private_mode(std::path::Path::new(path), 0o400);
            }
            "missing" => std::fs::remove_file(&*path).unwrap(),
            "fifo" => {
                std::fs::remove_file(&*path).unwrap();
                let name = std::ffi::CString::new(path.as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o400) }, 0);
            }
            "symlink" => {
                let alias = fixture.root.join("inputs/linked.png");
                std::os::unix::fs::symlink(&*path, &alias).unwrap();
                *path = alias.to_str().unwrap().into();
            }
            "alias" => *path = path.replace("/inputs/", "/inputs/./"),
            _ => unreachable!(),
        }
        let (result, events) = invoke(&fixture, request(vec![attachment], false));
        assert!(
            matches!(
                result,
                Err(AgentError::AttachmentChanged | AgentError::AttachmentUnavailable)
            ),
            "{variant}: {result:?}"
        );
        assert!(events.is_empty());
        assert!(!fixture.root.join("staged.json").exists());
    }
}
#[test]
fn 过多超大未知附件在读取或启动前拒绝() {
    let fixture = fixture("normal", "codex_cli");
    let input = image(&fixture, "image/png");
    assert_eq!(
        invoke(&fixture, request(vec![input.clone(); 4], false))
            .0
            .unwrap_err(),
        AgentError::AttachmentInvalid
    );
    assert!(!fixture.calls.exists());
    let mut big = input.clone();
    let Attachment::Image { size, .. } = &mut big;
    *size = 4 * 1024 * 1024 + 1;
    assert_eq!(
        invoke(&fixture, request(vec![big], false)).0.unwrap_err(),
        AgentError::AttachmentInvalid
    );
    assert!(!fixture.calls.exists());
    for attachment in [
        serde_json::json!({"kind":"url","url":"https://example.invalid/private.png"}),
        serde_json::json!({"kind":"image","path":"/no-file","mediaType":"image/png","size":1,"sha256":"a".repeat(64),"command":"private"}),
    ] {
        assert!(
            serde_json::from_value::<iquji_shared_agent::TurnRequest>(
                serde_json::json!({"agentId":"fixture","prompt":"问题","attachments":[attachment]})
            )
            .is_err()
        );
    }
}
#[test]
fn 源文件后续改写不改变冻结输入且暂存被篡改不能成功() {
    let fixture = fixture("mutate-source", "codex_cli");
    let input = image(&fixture, "image/png");
    assert!(invoke(&fixture, request(vec![input], false)).0.is_ok());
    assert_staged_removed(&fixture);
    let fixture = support::images::fixture("mutate-stage", "codex_cli");
    let input = image(&fixture, "image/png");
    let (result, events) = invoke(&fixture, request(vec![input], false));
    assert_eq!(result.unwrap_err(), AgentError::AttachmentChanged);
    assert!(!events.iter().any(|v| v["kind"] == "text_delta"));
    assert_staged_removed(&fixture);
}
#[test]
fn 图片回合超时或取消仍清理暂存且不清除调用者输入() {
    for cancel in [false, true] {
        let fixture = fixture("hang", "codex_cli");
        let input = image(&fixture, "image/png");
        let mut config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&fixture.config).unwrap()).unwrap();
        config["turnTimeoutMs"] = serde_json::json!(if cancel { 3000 } else { 1200 });
        fixture.save(config);
        std::fs::write(
            fixture.root.join("expected.json"),
            serde_json::to_vec(&vec![input.clone()]).unwrap(),
        )
        .unwrap();
        let cancelled = AtomicBool::new(false);
        let result = std::thread::scope(|scope| {
            if cancel {
                scope.spawn(|| {
                    support::codex::wait_file(&fixture.root.join("staged.json"));
                    cancelled.store(true, Ordering::SeqCst);
                });
            }
            iquji_shared_agent::run_turn(
                &iquji_shared_agent::Configuration::load(&fixture.config).unwrap(),
                request(vec![input], false),
                &cancelled,
                |_| {},
            )
        });
        assert_eq!(
            result.unwrap_err(),
            if cancel {
                AgentError::TurnCancelled
            } else {
                AgentError::TurnTimedOut
            }
        );
        assert_staged_removed(&fixture);
        assert!(fixture.root.join("inputs/图片;literal.png").exists());
    }
}

#[test]
fn 单次标准输入入口传递附件但输出不携带本机路径() {
    use std::io::Write;
    for adapter in ["codex_cli", "native_acp"] {
        let fixture = fixture("normal", adapter);
        let attachments = vec![image(&fixture, "image/png")];
        std::fs::write(
            fixture.root.join("expected.json"),
            serde_json::to_vec(&attachments).unwrap(),
        )
        .unwrap();
        let mut child = support::codex::command(&fixture).spawn().unwrap();
        child.stdin.take().unwrap().write_all(&serde_json::to_vec(&serde_json::json!({"agentId":"fixture","prompt":"请描述图片","attachments":attachments})).unwrap()).unwrap();
        let output = support::codex::bounded(child);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            support::codex::frames(&output).last().unwrap()["text"],
            "图片已收到"
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains(fixture.root.to_str().unwrap()));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private-"));
        assert_staged_removed(&fixture);
    }
}

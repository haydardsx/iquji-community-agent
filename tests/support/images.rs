use super::{Fixture, private_mode, quote};
use base64::Engine as _;
use iquji_shared_agent::{
    AgentError, Attachment, Configuration, TurnRequest, TurnResult, run_turn,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{BufRead, Write};
use std::os::fd::FromRawFd;
use std::sync::{Arc, Mutex, atomic::AtomicBool};
use std::time::Duration;

pub const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGNgYGAAAAAEAAH2FzhVAAAAAElFTkSuQmCC";
pub const JPEG: &str = "/9j/4AAQSkZJRgABAQAASABIAAD/4QBMRXhpZgAATU0AKgAAAAgAAYdpAAQAAAABAAAAGgAAAAAAA6ABAAMAAAABAAEAAKACAAQAAAABAAAAAaADAAQAAAABAAAAAQAAAAD/7QA4UGhvdG9zaG9wIDMuMAA4QklNBAQAAAAAAAA4QklNBCUAAAAAABDUHYzZjwCyBOmACZjs+EJ+/8AAEQgAAQABAwEiAAIRAQMRAf/EAB8AAAEFAQEBAQEBAAAAAAAAAAABAgMEBQYHCAkKC//EALUQAAIBAwMCBAMFBQQEAAABfQECAwAEEQUSITFBBhNRYQcicRQygZGhCCNCscEVUtHwJDNicoIJChYXGBkaJSYnKCkqNDU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6g4SFhoeIiYqSk5SVlpeYmZqio6Slpqeoqaqys7S1tre4ubrCw8TFxsfIycrS09TV1tfY2drh4uPk5ebn6Onq8fLz9PX29/j5+v/EAB8BAAMBAQEBAQEBAQEAAAAAAAABAgMEBQYHCAkKC//EALURAAIBAgQEAwQHBQQEAAECdwABAgMRBAUhMQYSQVEHYXETIjKBCBRCkaGxwQkjM1LwFWJy0QoWJDThJfEXGBkaJicoKSo1Njc4OTpDREVGR0hJSlNUVVZXWFlaY2RlZmdoaWpzdHV2d3h5eoKDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uLj5OXm5+jp6vLz9PX29/j5+v/bAEMAAgICAgICAwICAwUDAwMFBgUFBQUGCAYGBgYGCAoICAgICAgKCgoKCgoKCgwMDAwMDA4ODg4ODw8PDw8PDw8PD//bAEMBAgICBAQEBwQEBxALCQsQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEP/dAAQAAf/aAAwDAQACEQMRAD8A/n/ooooA/9k=";

pub fn fixture(mode: &str, adapter: &str) -> Fixture {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.root.join("must-not-read"),
        "private-file-must-not-escape",
    )
    .unwrap();
    let first = if mode == "missing-first" {
        ""
    } else {
        "--image"
    };
    let resume = if mode == "missing-resume" {
        ""
    } else {
        "--image"
    };
    fixture.script(&format!(r#"
case "$*" in
 --version) printf 'codex-cli 0.50.0\n';;
 'exec --help') printf '%s\n' '--json --color --sandbox --skip-git-repo-check -C resume {first}';;
 'exec resume --help') printf '%s\n' '--json {resume}';;
 *) printf '%s\000' "$@" > {args}
    SHARED_IMAGE_MODE='{mode}' SHARED_IMAGE_ADAPTER='{adapter}' SHARED_IMAGE_ROOT={root} exec {peer} --exact image_peer_fixture --nocapture 3>&1 1>/dev/null;;
esac
"#,args=quote(&fixture.root.join("arguments")),root=quote(&fixture.root),peer=quote(&std::env::current_exe().unwrap())));
    let mut config = fixture.document();
    config["agents"][0]["adapter"] = json!(adapter);
    config["turnTimeoutMs"] = json!(3000);
    fixture.save(config);
    fixture
}
pub fn image(fixture: &Fixture, media: &str) -> Attachment {
    let directory = fixture.root.join("inputs");
    if !directory.exists() {
        std::fs::create_dir(&directory).unwrap();
        private_mode(&directory, 0o700);
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(if media == "image/png" { PNG } else { JPEG })
        .unwrap();
    let path = directory.join(if media == "image/png" {
        "图片;literal.png"
    } else {
        "照片.jpg"
    });
    std::fs::write(&path, &bytes).unwrap();
    private_mode(&path, 0o400);
    Attachment::Image {
        path: path.to_str().unwrap().into(),
        media_type: media.into(),
        size: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    }
}
pub fn request(images: Vec<Attachment>, resume: bool) -> TurnRequest {
    TurnRequest {
        workspace_write: false,
        agent_id: "fixture".into(),
        context_fingerprint: None,
        prompt: "请描述图片".into(),
        session_id: resume.then(|| "thread-image".into()),
        model: None,
        attachments: images,
        mcp_servers: Vec::new(),
    }
}
pub fn invoke(
    fixture: &Fixture,
    request: TurnRequest,
) -> (Result<TurnResult, AgentError>, Vec<Value>) {
    std::fs::write(
        fixture.root.join("expected.json"),
        serde_json::to_vec(&request.attachments).unwrap(),
    )
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let record = events.clone();
    let result = run_turn(
        &Configuration::load(&fixture.config).unwrap(),
        request,
        &AtomicBool::new(false),
        move |event| {
            record
                .lock()
                .unwrap()
                .push(serde_json::to_value(event).unwrap())
        },
    );
    let events = events.lock().unwrap().clone();
    (result, events)
}
fn frame(pipe: &mut std::fs::File, value: Value) {
    serde_json::to_writer(&mut *pipe, &value).unwrap();
    pipe.write_all(b"\n").unwrap();
    pipe.flush().unwrap();
}
fn read(input: &mut impl BufRead) -> Value {
    let mut line = String::new();
    assert!(input.read_line(&mut line).unwrap() > 0);
    serde_json::from_str(&line).unwrap()
}

pub fn peer() {
    let Ok(mode) = std::env::var("SHARED_IMAGE_MODE") else {
        return;
    };
    let root = std::path::PathBuf::from(std::env::var_os("SHARED_IMAGE_ROOT").unwrap());
    let adapter = std::env::var("SHARED_IMAGE_ADAPTER").unwrap();
    let expected: Vec<Attachment> =
        serde_json::from_slice(&std::fs::read(root.join("expected.json")).unwrap()).unwrap();
    let mut pipe = unsafe { std::fs::File::from_raw_fd(3) };
    if adapter == "codex_cli" {
        let args = std::fs::read(root.join("arguments")).unwrap();
        let args: Vec<_> = args
            .split(|b| *b == 0)
            .filter(|part| !part.is_empty())
            .map(|v| String::from_utf8(v.to_vec()).unwrap())
            .collect();
        let start = args.iter().position(|a| a == "--image").unwrap();
        let resumed = args.iter().any(|a| a == "resume");
        assert_eq!(
            args[start - 1],
            if resumed {
                "resume".to_owned()
            } else {
                root.join("workspace").to_str().unwrap().to_owned()
            }
        );
        let mut staged = Vec::new();
        for (index, image) in expected.iter().enumerate() {
            assert_eq!(args[start + index * 2], "--image");
            let path = std::path::PathBuf::from(&args[start + index * 2 + 1]);
            let Attachment::Image {
                path: source,
                size,
                sha256,
                ..
            } = image;
            assert_ne!(path.as_path(), std::path::Path::new(source));
            let data = std::fs::read(&path).unwrap();
            assert_eq!(data.len() as u64, *size);
            assert_eq!(format!("{:x}", Sha256::digest(&data)), *sha256);
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
                0o400
            );
            assert_eq!(
                std::fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777,
                0o700
            );
            if mode == "mutate-source" {
                private_mode(std::path::Path::new(source), 0o600);
                std::fs::write(source, b"source replaced after snapshot").unwrap();
            }
            if mode == "mutate-stage" {
                private_mode(&path, 0o600);
                std::fs::write(&path, b"altered CLI input").unwrap();
            }
            staged.push(path);
        }
        assert_eq!(
            &args[start + expected.len() * 2..],
            if resumed {
                vec!["thread-image", "-"]
            } else {
                vec!["-"]
            }
        );
        std::fs::write(
            root.join("staged.json"),
            serde_json::to_vec(&staged).unwrap(),
        )
        .unwrap();
        frame(
            &mut pipe,
            json!({"type":"thread.started","thread_id":"thread-image"}),
        );
        frame(&mut pipe, json!({"type":"turn.started"}));
        if mode == "hang" {
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        frame(
            &mut pipe,
            json!({"type":"item.completed","item":{"type":"agent_message","text":"图片已收到"}}),
        );
        frame(&mut pipe, json!({"type":"turn.completed"}));
        return;
    }
    let mut input = std::io::stdin().lock();
    let init = read(&mut input);
    assert_eq!(init["method"], "initialize");
    assert_ne!(
        init["params"]["clientCapabilities"]["fs"]["readTextFile"],
        true
    );
    assert_ne!(
        init["params"]["clientCapabilities"]["fs"]["writeTextFile"],
        true
    );
    frame(
        &mut pipe,
        json!({"jsonrpc":"2.0","id":init["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"promptCapabilities":{"image":mode!="unsupported"}},"authMethods":[]}}),
    );
    if mode == "unsupported" {
        let mut ignored = String::new();
        input.read_line(&mut ignored).unwrap();
        assert!(ignored.is_empty());
        return;
    }
    let setup = read(&mut input);
    assert!(matches!(
        setup["method"].as_str(),
        Some("session/new" | "session/load")
    ));
    frame(
        &mut pipe,
        json!({"jsonrpc":"2.0","id":setup["id"],"result":if setup["method"]=="session/new"{json!({"sessionId":"thread-image"})}else{json!({})}}),
    );
    let prompt = read(&mut input);
    let contents = prompt["params"]["prompt"].as_array().unwrap();
    assert_eq!(contents.len(), expected.len() + 1);
    assert_eq!(contents[0], json!({"type":"text","text":"请描述图片"}));
    for (content, descriptor) in contents[1..].iter().zip(expected) {
        let Attachment::Image {
            media_type,
            size,
            sha256,
            ..
        } = descriptor;
        assert_eq!(content["type"], "image");
        assert_eq!(content["mimeType"], media_type);
        assert!(content.get("uri").is_none());
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(content["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(bytes.len() as u64, size);
        assert_eq!(format!("{:x}", Sha256::digest(bytes)), sha256);
    }
    std::fs::write(
        root.join("prompt.json"),
        serde_json::to_vec(&prompt).unwrap(),
    )
    .unwrap();
    // 图片字节不授权额外文件读取；即使对方主动请求也不能得到本机内容。
    frame(
        &mut pipe,
        json!({"jsonrpc":"2.0","id":900,"method":"fs/read_text_file","params":{"sessionId":"thread-image","path":root.join("must-not-read")}}),
    );
    let response = read(&mut input);
    assert_eq!(response["id"], 900);
    assert!(response.get("error").is_some());
    assert!(response.get("result").is_none());
    frame(
        &mut pipe,
        json!({"jsonrpc":"2.0","id":901,"method":"fs/write_text_file","params":{"sessionId":"thread-image","path":root.join("must-not-write"),"content":"forbidden"}}),
    );
    let response = read(&mut input);
    assert_eq!(response["id"], 901);
    assert!(response.get("error").is_some());
    assert!(!root.join("must-not-write").exists());
    frame(
        &mut pipe,
        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"thread-image","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"图片已收到"}}}}),
    );
    frame(
        &mut pipe,
        json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"end_turn"}}),
    );
    std::thread::sleep(Duration::from_secs(1));
}
pub fn assert_staged_removed(fixture: &Fixture) {
    if fixture.root.join("staged.json").exists() {
        let paths: Vec<std::path::PathBuf> =
            serde_json::from_slice(&std::fs::read(fixture.root.join("staged.json")).unwrap())
                .unwrap();
        for path in paths {
            assert!(!path.exists());
            assert!(!path.parent().unwrap().exists());
        }
    }
}

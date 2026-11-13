use super::*;
struct ModelOverride(crate::private_directory::PrivateDirectory);
impl ModelOverride {
    fn new() -> Result<Self, AgentError> {
        crate::private_directory::PrivateDirectory::new().map(Self)
    }
    fn root(&self) -> &Path {
        self.0.path()
    }
    fn patch(&self) -> std::path::PathBuf {
        self.root().join("model.patch.json")
    }
    fn accept(
        &mut self,
        descriptor: PreparedModelOverride,
        cancelled: &AtomicBool,
        deadline: Instant,
    ) -> Result<(), AgentError> {
        self.0.verify()?;
        super::verify(self.root(), &descriptor, cancelled, deadline)?;
        self.0.verify()
    }
}
use std::os::unix::fs::{PermissionsExt, symlink};
use std::time::Duration;

fn descriptor(bytes: &[u8]) -> PreparedConfigurationFile {
    PreparedConfigurationFile {
        size: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(bytes)),
    }
}

fn prepared(snapshot: &ModelOverride) -> PreparedModelOverride {
    let bytes = b"[]";
    std::fs::write(snapshot.patch(), bytes).unwrap();
    std::fs::set_permissions(snapshot.patch(), std::fs::Permissions::from_mode(0o600)).unwrap();
    PreparedModelOverride {
        model: "relay/next".into(),
        patch: descriptor(bytes),
        settings: None,
    }
}

#[test]
fn 私有覆盖验证摘要权限并在成功后清理自己创建的目录() {
    let mut snapshot = ModelOverride::new().unwrap();
    let root = snapshot.root().to_path_buf();
    let metadata = prepared(&snapshot);
    snapshot
        .accept(
            metadata,
            &AtomicBool::new(false),
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    drop(snapshot);
    assert!(!root.exists());
}

#[test]
fn 内容变化共享权限及未声明文件不能被当作正式覆盖() {
    for case in ["changed", "permission", "extra", "missing"] {
        let mut snapshot = ModelOverride::new().unwrap();
        let root = snapshot.root().to_path_buf();
        let metadata = prepared(&snapshot);
        match case {
            "changed" => std::fs::write(snapshot.patch(), b"{}").unwrap(),
            "permission" => {
                std::fs::set_permissions(snapshot.patch(), std::fs::Permissions::from_mode(0o640))
                    .unwrap()
            }
            "extra" => std::fs::write(root.join("unrequested"), b"extra").unwrap(),
            "missing" => std::fs::remove_file(snapshot.patch()).unwrap(),
            _ => unreachable!(),
        }
        assert_eq!(
            snapshot.accept(
                metadata,
                &AtomicBool::new(false),
                Instant::now() + Duration::from_secs(1)
            ),
            Err(AgentError::ConfigurationMetadataInvalid)
        );
        drop(snapshot);
        assert!(!root.exists());
    }
}

#[test]
fn 拒绝符号链接且清理不删除外部目标() {
    let mut snapshot = ModelOverride::new().unwrap();
    let outside = ModelOverride::new().unwrap();
    std::fs::write(outside.root().join("keep"), b"outside").unwrap();
    symlink(outside.root().join("keep"), snapshot.patch()).unwrap();
    let metadata = PreparedModelOverride {
        model: "relay/next".into(),
        patch: descriptor(b"outside"),
        settings: None,
    };
    assert_eq!(
        snapshot.accept(
            metadata,
            &AtomicBool::new(false),
            Instant::now() + Duration::from_secs(1)
        ),
        Err(AgentError::ConfigurationMetadataInvalid)
    );
    drop(snapshot);
    assert_eq!(
        std::fs::read(outside.root().join("keep")).unwrap(),
        b"outside"
    );
}

#[test]
fn 目录被替换时按原句柄清理受管文件而不递归删除替换目标() {
    let mut snapshot = ModelOverride::new().unwrap();
    let outside = ModelOverride::new().unwrap();
    let metadata = prepared(&snapshot);
    let original = snapshot.root().with_extension("moved");
    std::fs::rename(snapshot.root(), &original).unwrap();
    symlink(outside.root(), snapshot.root()).unwrap();
    let replacement = snapshot.root().to_path_buf();
    std::fs::write(outside.root().join("keep"), b"outside").unwrap();
    assert_eq!(
        snapshot.accept(
            metadata,
            &AtomicBool::new(false),
            Instant::now() + Duration::from_secs(1)
        ),
        Err(AgentError::ConfigurationMetadataInvalid)
    );
    drop(snapshot);
    assert!(!original.join("model.patch.json").exists());
    assert_eq!(
        std::fs::read(outside.root().join("keep")).unwrap(),
        b"outside"
    );
    std::fs::remove_file(replacement).unwrap();
    std::fs::remove_dir(original).unwrap();
}

#[test]
fn 验证取消和预算耗尽仍清理私有覆盖() {
    for cancelled in [false, true] {
        let mut snapshot = ModelOverride::new().unwrap();
        let root = snapshot.root().to_path_buf();
        let metadata = prepared(&snapshot);
        let cancellation = AtomicBool::new(false);
        if cancelled {
            cancellation.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        assert_eq!(
            snapshot.accept(metadata, &cancellation, Instant::now()),
            Err(if cancelled {
                AgentError::TurnCancelled
            } else {
                AgentError::TurnTimedOut
            })
        );
        drop(snapshot);
        assert!(!root.exists());
    }
}

#![allow(dead_code)]

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
pub const CODEX: &str = r#"
case "$*" in
  --version) printf 'codex-cli 0.50.0\n';;
  'exec --help') printf '%s\n' '--json --color --sandbox --skip-git-repo-check -C resume --image';;
  'exec resume --help') printf '%s\n' '--json --image';;
  *) exit 42;;
esac
"#;

pub struct Fixture {
    pub root: PathBuf,
    pub workspace: PathBuf,
    pub program: PathBuf,
    pub config: PathBuf,
    pub calls: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        static TOOL_WARMUP: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        TOOL_WARMUP.get_or_init(|| {
            // 时序回归从已安装制品开始，避免操作系统首次扫描被计入进程管道监管。
            let status = Command::new(env!("CARGO_BIN_EXE_iquji-shared-agent"))
                .arg("--fixture-warmup")
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(2));
        });
        let root = std::env::temp_dir().join(format!(
            "shared-agent-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        private_mode(&root, 0o700);
        let workspace = root.join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let fixture = Self {
            program: root.join("fake agent;literal"),
            config: root.join("config.json"),
            calls: root.join("calls"),
            workspace,
            root,
        };
        fixture.script(CODEX);
        fixture.save(fixture.document());
        fixture
    }

    pub fn document(&self) -> Value {
        json!({"schemaVersion":1, "workingDirectory":self.workspace, "timeoutMs":500, "maxOutputBytes":16384,
            "agents":[{"id":"fixture", "adapter":"codex_cli", "executable":self.program}]})
    }

    pub fn save(&self, document: Value) {
        std::fs::write(&self.config, serde_json::to_vec(&document).unwrap()).unwrap();
        private_mode(&self.config, 0o600);
    }

    pub fn script(&self, body: &str) {
        std::fs::write(&self.program, format!("#!/bin/sh\nif [ \"$1\" = '--fixture-warmup' ]; then exit 0; fi\nprintf '%s\\n' \"$*\" >> {}\n{body}", quote(&self.calls))).unwrap();
        private_mode(&self.program, 0o700);
        // 新文件首次执行可能等待系统扫描；超时回归只测已启动进程的监管边界。
        assert!(
            Command::new(&self.program)
                .arg("--fixture-warmup")
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .status()
                .unwrap()
                .success()
        );
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_iquji-shared-agent"));
        command
            .args([
                std::ffi::OsStr::new("--config"),
                self.config.as_os_str(),
                std::ffi::OsStr::new("scan"),
            ])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .current_dir(&self.root);
        command
    }

    pub fn report(&self) -> Value {
        let output = self.command().output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        serde_json::from_slice(&output.stdout).unwrap()
    }

    pub fn assert_error(&self, expected: &str) {
        if expected == "probe_timed_out" {
            let output = self.command().output().unwrap();
            if output.status.code() == Some(2) {
                // 整体期限已耗尽时不能为序列化报告另开预算，只交付稳定诊断。
                assert!(output.stdout.is_empty());
                assert_eq!(output.stderr, b"probe_timed_out\n");
            } else {
                assert!(output.status.success());
                let report: Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(report["connections"][0]["error"], expected);
            }
            return;
        }
        assert_eq!(self.report()["connections"][0]["error"], expected);
    }

    pub fn assert_configuration_error(&self, output: Output, expected: &str) {
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            format!("{expected}\n")
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub fn private_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

pub fn quote(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

pub mod codex;
pub mod deepseek;

pub const CONFIGURATION_OVERRIDES: &[&str] = &[
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "ALLUSERSPROFILE",
    "CFFIXED_USER_HOME",
    "XDG_CONFIG_HOME",
    "XDG_CONFIG_DIRS",
    "XDG_DATA_HOME",
    "XDG_DATA_DIRS",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "XDG_RUNTIME_DIR",
];

pub fn assert_configuration_overrides_absent() {
    for name in CONFIGURATION_OVERRIDES {
        assert!(
            std::env::var_os(name).is_none(),
            "环境不得覆盖隔离配置: {name}"
        );
    }
}
pub mod images;

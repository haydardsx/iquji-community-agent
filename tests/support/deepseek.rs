use super::{Fixture, quote};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

pub fn fixture(scenario: &str, timeout: u64, limit: usize) -> Fixture {
    let fixture = Fixture::new();
    let protected = super::CONFIGURATION_OVERRIDES
        .iter()
        .map(|name| format!("${{{name}+x}}"))
        .collect::<String>();
    fixture.script(&format!(r#"
if [ "{protected}" != "" ]; then exit 45; fi
case "$*" in
 --version|--help)
   printf '%s\t%s\t%s\n' "$HOME" "$DSH_HOME" "${{FAKE_PROVIDER_KEY-unset}}" >> {metadata}
   case "$*" in --version) printf '0.1.0-rc.6\n';; *) printf '%s\n' '--profile <name>';; esac;;
 *) printf '%s\000' "$@" > {arguments}
    SHARED_DSH_SCENARIO='{scenario}' SHARED_DSH_RECORD={record} exec {peer} --exact deepseek_peer_fixture --nocapture 3>&1 1>/dev/null;;
esac
"#,metadata=quote(&fixture.root.join("metadata")),arguments=quote(&fixture.root.join("arguments")),record=quote(&fixture.root.join("record")),peer=quote(&std::env::current_exe().unwrap())));
    let mut config = fixture.document();
    config["agents"][0]["adapter"] = json!("deepseek_harness");
    config["timeoutMs"] = json!(1000);
    config["turnTimeoutMs"] = json!(timeout);
    config["maxTurnOutputBytes"] = json!(limit);
    fixture.save(config);
    fixture
}
pub fn record(fixture: &Fixture) -> Value {
    serde_json::from_slice(&std::fs::read(fixture.root.join("record")).unwrap()).unwrap()
}
pub fn assert_cleaned(fixture: &Fixture) {
    for line in std::fs::read_to_string(fixture.root.join("metadata"))
        .unwrap()
        .lines()
    {
        let fields: Vec<_> = line.split('\t').collect();
        assert_eq!(fields[0], fields[1]);
        assert_eq!(fields[2], "unset");
        assert!(!std::path::Path::new(fields[0]).exists());
    }
    if fixture.root.join("record").exists() {
        let record = record(fixture);
        for name in ["home", "dshHome"] {
            let path = std::path::Path::new(record[name].as_str().unwrap());
            if !path.starts_with(&fixture.root) {
                assert!(!path.exists());
            }
        }
    }
}
pub fn assert_reaped(fixture: &Fixture) {
    let pid = record(fixture)["pid"].as_i64().unwrap() as i32;
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}
pub fn peer() {
    let Ok(scenario) = std::env::var("SHARED_DSH_SCENARIO") else {
        return;
    };
    let path = std::path::PathBuf::from(std::env::var_os("SHARED_DSH_RECORD").unwrap());
    super::assert_configuration_overrides_absent();
    let home = std::env::var("HOME").unwrap();
    let dsh_home = std::env::var("DSH_HOME").unwrap();
    std::fs::write(&path, serde_json::to_vec(&json!({"pid":std::process::id(),"home":home,"dshHome":dsh_home,"mode":std::env::var("DSH_PERMISSION_MODE").ok(),"provider":std::env::var("FAKE_PROVIDER_KEY").ok(),"deepseekKey":std::env::var("DEEPSEEK_API_KEY").ok(),"codexHome":std::env::var("CODEX_HOME").ok(),"homeMode":std::fs::metadata(&home).unwrap().permissions().mode() & 0o777,"dshMode":std::fs::metadata(&dsh_home).unwrap().permissions().mode() & 0o777})).unwrap()).unwrap();
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap();
    assert!(input.is_empty(), "headless的stdin必须直接关闭");
    let mut pipe = unsafe { std::fs::File::from_raw_fd(3) };
    match scenario.as_str() {
        "hang" => loop {
            std::thread::sleep(Duration::from_secs(1));
        },
        "flood" => loop {
            std::io::stderr().write_all(&[b'x'; 1024]).unwrap();
        },
        "credential" => {
            std::io::stderr()
                .write_all(b"dsh: MISSING_CREDENTIAL: private-provider secret-key\n")
                .unwrap();
            std::process::exit(1);
        }
        "credential-whitespace" => {
            std::io::stderr()
                .write_all(b"  dsh:   MISSING_CREDENTIAL : private-provider secret-key\n")
                .unwrap();
            std::process::exit(1);
        }
        "unknown-first" => {
            std::io::stderr()
                .write_all(b"dsh: UNKNOWN: private-provider\ndsh: MISSING_CREDENTIAL: secret-key\n")
                .unwrap();
            std::process::exit(1);
        }
        "failed" => {
            pipe.write_all(b"partial-not-success").unwrap();
            std::io::stderr()
                .write_all(b"dsh: UNKNOWN: private-provider secret-key\n")
                .unwrap();
            std::process::exit(7);
        }
        "invalid" => {
            pipe.write_all(&[0xff, 0xff]).unwrap();
        }
        "empty" => {
            pipe.write_all(b" \n\t").unwrap();
        }
        "large" => {
            pipe.write_all("答".repeat(100_000).as_bytes()).unwrap();
        }
        _ => {
            pipe.write_all("最终文本\n\t结束\u{001b}".as_bytes())
                .unwrap();
        }
    }
    pipe.flush().unwrap();
    if scenario == "detach" {
        use std::os::unix::process::CommandExt;
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "deepseek_detached_fixture", "--nocapture"])
            .env("SHARED_DSH_DETACHED", path.with_extension("detached"));
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        // 入口先退出，脱组后代由外层真实PID回归显式清理。
        #[allow(clippy::zombie_processes)]
        let _child = command.spawn().unwrap();
        super::codex::wait_file(&path.with_extension("detached"));
    }
}
pub fn detached_peer() {
    let Some(path) = std::env::var_os("SHARED_DSH_DETACHED") else {
        return;
    };
    std::fs::write(path, std::process::id().to_string()).unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

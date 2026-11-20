#![cfg(unix)]
mod support;

use iquji_shared_agent::AgentError;
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;
use support::{Fixture, codex::invoke_request, codex::request, private_mode, quote};

const CREDENTIALS: &str = "version: 1\nrefs: {}\nrecords: {}\n";

fn fixture(case: &str) -> Fixture {
    let fixture = Fixture::new();
    let home = fixture.root.join("dsh-home");
    std::fs::create_dir(&home).unwrap();
    private_mode(&home, 0o700);
    std::fs::write(home.join(".credentials.yaml"), CREDENTIALS).unwrap();
    private_mode(&home.join(".credentials.yaml"), 0o600);
    let mut document = fixture.document();
    document["timeoutMs"] = json!(1000);
    document["turnTimeoutMs"] = json!(3000);
    document["agents"][0]["adapter"] = json!("deepseek_harness");
    document["agents"][0]["deepseekConfigurationDirectory"] = json!(home);
    fixture.save(document);
    let mut dump = json!([
        {"id":"selection","name":"@deepseek-ai/dsh-agent-default-model","config":{"provider":"relay","model":"relay/chosen"}},
        {"id":"provider","name":"@deepseek-ai/dsh-llm-pi-ai","config":{"providers":{"relay":{"apiKeyEnv":"FAKE_DSH_KEY"}}}},
        {"id":"credentials","name":"@deepseek-ai/dsh-credentials-local","disabled":true},
        {"id":"authorization","name":"@deepseek-ai/dsh-authorization","disabled":true},
        {"id":"deepseek-account","name":"@deepseek-ai/dsh-deepseek-account","disabled":true}
    ]);
    match case {
        "replaced-credentials" => dump[2]["name"] = json!("custom-provider"),
        "replaced-account" => dump[4]["name"] = json!("custom-account"),
        "obsolete-account" => {
            dump[4]["name"] = json!("@deepseek-ai/dsh-deepseek-account-platform");
        }
        "duplicate-account" => {
            let duplicate = dump[4].clone();
            dump.as_array_mut().unwrap().push(duplicate);
        }
        "wrong-account-id" => dump[4]["id"] = json!("custom-account"),
        "official" => (),
        _ => unreachable!(),
    }
    let dump_path = fixture.root.join("dump.json");
    std::fs::write(&dump_path, dump.to_string()).unwrap();
    private_mode(&dump_path, 0o600);
    fixture.script(&format!(
        r#"case "$*" in
 --version) printf '0.2.2-rc.2\n';;
 --help) printf '%s\n' '--profile --dump-config --patch';;
 '--profile headless --dump-config') cat {dump};;
 *) test "$1" = '--profile' && test "$2" = 'headless' && test "$3" = '--patch' || exit 44
    test "$DSH_PERMISSION_MODE" = 'read-only' || exit 45
    test "$DSH_HOME" = {home} && test "$HOME" != "$DSH_HOME" || exit 46
    test "${{FAKE_DSH_KEY-unset}}" = unset || exit 47
    cat "$4" > {applied}
    printf '%s\n' "$4" > {path}
    printf 'ready-fixture';;
esac"#,
        dump = quote(&dump_path),
        home = quote(&home),
        applied = quote(&fixture.root.join("applied.json")),
        path = quote(&fixture.root.join("patch-path")),
    ));
    fixture
}

#[test]
fn 新版默认回合与显式型号共用私有认证覆盖且保留第三方配置() {
    for model in [None, Some("relay/override")] {
        let fixture = fixture("official");
        let original = std::fs::read(&fixture.config).unwrap();
        let mut request = request(None);
        request.model = model.map(str::to_owned);
        let (result, events) = invoke_request(&fixture, request, &AtomicBool::new(false));
        assert_eq!(result.unwrap().text, "ready-fixture");
        assert!(events.is_empty());
        let mut expected = vec![
            json!({"id":"credentials","disabled":false}),
            json!({"id":"authorization","disabled":false}),
            json!({"id":"deepseek-account","disabled":false}),
        ];
        if let Some(model) = model {
            expected.push(json!({"id":"selection","config":{"provider":"relay","model":model}}));
        }
        let applied: Value =
            serde_json::from_slice(&std::fs::read(fixture.root.join("applied.json")).unwrap())
                .unwrap();
        assert_eq!(applied, Value::Array(expected));
        let patch = std::fs::read_to_string(fixture.root.join("patch-path")).unwrap();
        assert!(!std::path::Path::new(patch.trim()).exists());
        assert_eq!(std::fs::read(&fixture.config).unwrap(), original);
        assert_eq!(
            std::fs::read_to_string(fixture.root.join("dsh-home/.credentials.yaml")).unwrap(),
            CREDENTIALS
        );
    }
}

#[test]
fn 新版认证条目被替换时在执行真实回合前拒绝() {
    for case in [
        "replaced-credentials",
        "replaced-account",
        "obsolete-account",
        "duplicate-account",
        "wrong-account-id",
    ] {
        let fixture = fixture(case);
        let (result, events) = invoke_request(&fixture, request(None), &AtomicBool::new(false));
        assert_eq!(
            result.unwrap_err(),
            AgentError::ConfigurationMetadataInvalid,
            "{case}"
        );
        assert!(events.is_empty(), "{case}");
        assert!(!fixture.root.join("applied.json").exists(), "{case}");
        assert_eq!(
            std::fs::read_to_string(fixture.root.join("dsh-home/.credentials.yaml")).unwrap(),
            CREDENTIALS,
            "{case}"
        );
    }
}

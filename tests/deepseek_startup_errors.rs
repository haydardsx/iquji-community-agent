#![cfg(unix)]
mod support;

use serde_json::json;
use support::{Fixture, codex::cli, codex::frames, quote};

#[test]
fn 官方凭据非字符串启动错误精确投影且不泄露原文() {
    let header = "TypeError: credentials-local: the value for \"FAKE_PROVIDER_KEY\" in /private-user/.dsh/.credentials.yaml must be a string";
    let stack = "    at parseCredentialsDocument (file:///private-install/node_modules/@deepseek-ai/dsh-credentials-local/lib/index.js:132:40)\n    at LocalCredentialProvider.loadInitial (file:///private-install/node_modules/@deepseek-ai/dsh-credentials-local/lib/index.js:346:17)";
    for (diagnostic, expected) in [
        (
            format!("{header}\n{stack}\nprivate-secret-marker\n"),
            "configuration_metadata_invalid",
        ),
        (format!("{header}\n"), "agent_request_failed"),
        (
            format!("TypeError: unrelated startup failure\n{stack}\n"),
            "agent_request_failed",
        ),
    ] {
        let fixture = Fixture::new();
        let error = fixture.root.join("private-error");
        std::fs::write(&error, diagnostic).unwrap();
        let mut config = fixture.document();
        config["agents"][0]["adapter"] = json!("deepseek_harness");
        config["timeoutMs"] = json!(1000);
        config["turnTimeoutMs"] = json!(3000);
        fixture.save(config);
        fixture.script(&format!(
            "case \"$*\" in --version) printf '0.1.0-rc.6\\n';; --help) printf '%s\\n' '--profile';; *) cat {} >&2; exit 1;; esac",
            quote(&error)
        ));
        let output = cli(&fixture, &[]);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(output.stderr, format!("{expected}\n").as_bytes());
        assert_eq!(
            frames(&output),
            vec![json!({"kind":"turn_error","error":expected})]
        );
        for secret in [
            "private-user",
            "private-install",
            "private-secret-marker",
            "FAKE_PROVIDER_KEY",
        ] {
            assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
            assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
        }
    }
}

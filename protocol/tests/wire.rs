use iquji_agent_wire::*;
use serde_json::{Value, json};

fn request() -> Value {
    json!({"agentId":"fixture-agent", "prompt":"你好，😀", "sessionId":null})
}

#[test]
fn neutral_request_round_trips_and_absent_attachments_stay_empty() {
    let parsed = decode_turn_request(&serde_json::to_vec(&request()).unwrap()).unwrap();
    assert!(parsed.attachments.is_empty());
    assert!(parsed.mcp_servers.is_empty());
    assert_eq!(parsed.prompt, "你好，😀");
    assert!(
        serde_json::to_value(&parsed)
            .unwrap()
            .get("mcpServers")
            .is_none()
    );
    assert_eq!(
        decode_turn_request(&serde_json::to_vec(&parsed).unwrap()),
        Ok(parsed)
    );
}

#[test]
fn mcp_servers_are_bounded_loopback_descriptors_and_round_trip() {
    let mut input = request();
    let server = json!({
        "name": "iquji_studio_search",
        "url": format!("http://127.0.0.1:43123/mcp/{}", "a".repeat(48)),
    });
    input["mcpServers"] = json!([server]);
    let parsed = decode_turn_request(&serde_json::to_vec(&input).unwrap()).unwrap();
    assert_eq!(parsed.mcp_servers.len(), 1);
    assert_eq!(
        serde_json::to_value(&parsed).unwrap()["mcpServers"],
        input["mcpServers"]
    );

    let invalid_servers = [
        json!([{"name":"", "url":"http://127.0.0.1:43123/mcp/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]),
        json!([{"name":"name-with-dash", "url":"http://127.0.0.1:43123/mcp/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]),
        json!([{"name":"x".repeat(MAX_MCP_SERVER_NAME_BYTES + 1), "url":"http://127.0.0.1:43123/mcp/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]),
        json!([{"name":"server", "url":"https://127.0.0.1:43123/mcp/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]),
        json!([{"name":"server", "url":"http://localhost:43123/mcp/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]),
        json!([{"name":"server", "url":"http://0.0.0.0:43123/mcp/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]),
        json!([{"name":"server", "url":"http://127.0.0.1/mcp/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]),
        json!([{"name":"server", "url":"http://user@127.0.0.1:43123/mcp/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]),
        json!([{"name":"server", "url":"http://127.0.0.1:43123/mcp/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa?token=secret"}]),
        json!([{"name":"server", "url":"http://127.0.0.1:43123/mcp/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa#fragment"}]),
        json!([{"name":"server", "url":"http://127.0.0.1:43123/other/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]),
        json!([{"name":"server", "url":"http://127.0.0.1:43123/mcp/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa!"}]),
        json!([{"name":"server", "url":"x".repeat(MAX_MCP_SERVER_URL_BYTES + 1)}]),
    ];
    for servers in invalid_servers {
        let mut invalid = request();
        invalid["mcpServers"] = servers;
        assert_eq!(
            decode_turn_request(&serde_json::to_vec(&invalid).unwrap()),
            Err(AgentError::RequestInvalid)
        );
    }

    let mut invalid = request();
    invalid["mcpServers"] = json!([
        {"name":"same", "url":"http://127.0.0.1:43123/mcp/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
        {"name":"same", "url":"http://127.0.0.1:43124/mcp/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}
    ]);
    assert_eq!(
        decode_turn_request(&serde_json::to_vec(&invalid).unwrap()),
        Err(AgentError::RequestInvalid)
    );

    let mut too_many = request();
    too_many["mcpServers"] = json!(
        (0..=MAX_MCP_SERVERS)
            .map(|index| {
                json!({
                    "name": format!("server_{index}"),
                    "url": format!("http://127.0.0.1:{}/mcp/{}", 43123 + index, "a".repeat(48)),
                })
            })
            .collect::<Vec<_>>()
    );
    assert_eq!(
        decode_turn_request(&serde_json::to_vec(&too_many).unwrap()),
        Err(AgentError::RequestInvalid)
    );
}

#[test]
fn request_rejects_unknown_duplicate_trailing_and_unbounded_inputs() {
    let mut extra = request();
    extra["environment"] = json!({"SECRET":"value"});
    for bytes in [
        serde_json::to_vec(&extra).unwrap(),
        br#"{"agentId":"a","agentId":"b","prompt":"hello"}"#.to_vec(),
        [serde_json::to_vec(&request()).unwrap(), b" {}".to_vec()].concat(),
        vec![b' '; MAX_REQUEST_BYTES + 1],
    ] {
        assert_eq!(decode_turn_request(&bytes), Err(AgentError::RequestInvalid));
    }
    for (field, value) in [
        ("prompt", json!(" ")),
        ("prompt", json!("x".repeat(MAX_PROMPT_BYTES + 1))),
        ("sessionId", json!("bad\nidentity")),
        ("agentId", json!("wrong/id")),
    ] {
        let mut invalid = request();
        invalid[field] = value;
        assert_eq!(
            decode_turn_request(&serde_json::to_vec(&invalid).unwrap()),
            Err(AgentError::RequestInvalid)
        );
    }
}

#[test]
fn attachments_validate_descriptors_without_opening_or_authorizing_paths() {
    let image = Attachment::Image {
        path: "/not-present/private/image.png".into(),
        media_type: "image/png".into(),
        size: 10,
        sha256: "a".repeat(64),
    };
    // 文件不存在也可通过纯描述验证；实际读取和权限复核属于执行器。
    assert_eq!(validate_attachments(std::slice::from_ref(&image)), Ok(()));
    assert_eq!(
        validate_attachments(&vec![image.clone(); MAX_IMAGES + 1]),
        Err(AgentError::AttachmentInvalid)
    );
    for (field, value) in [
        ("path", json!("relative.png")),
        ("path", json!("/../escape.png")),
        ("size", json!(MAX_IMAGE_BYTES + 1)),
        ("mediaType", json!("image/svg+xml")),
        ("sha256", json!("sha256:abc")),
        ("kind", json!("url")),
        ("command", json!("anything")),
    ] {
        let mut invalid = serde_json::to_value(&image).unwrap();
        invalid[field] = value;
        let mut input = request();
        input["attachments"] = json!([invalid]);
        assert!(decode_turn_request(&serde_json::to_vec(&input).unwrap()).is_err());
    }
}

#[test]
fn event_and_terminal_share_the_canonical_host_decoder() {
    let values = [
        json!({"kind":"session_started","sessionId":"real-thread","resumable":true}),
        json!({"kind":"text_delta","text":"完成"}),
        json!({"kind":"turn_completed","sessionId":"real-thread","resumable":true,"text":"完成","stopReason":"end_turn"}),
        json!({"kind":"turn_error","error":"turn_timed_out"}),
    ];
    for value in values {
        let parsed = decode_turn_output(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), value);
    }
}

#[test]
fn single_turn_result_has_no_fabricated_session() {
    let value =
        json!({"kind":"turn_completed","resumable":false,"text":"结果","stopReason":"end_turn"});
    let parsed = decode_turn_output(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
    let mut inconsistent = value;
    inconsistent["resumable"] = json!(true);
    assert_eq!(
        decode_turn_output(&serde_json::to_vec(&inconsistent).unwrap()),
        Err(AgentError::ProtocolInvalid)
    );
}

#[test]
fn output_refuses_unknown_codes_fields_and_nonframes() {
    for value in [
        json!({"kind":"turn_error","error":"raw_private_cause"}),
        json!({"kind":"turn_completed","sessionId":null,"resumable":false,"text":"hello","stopReason":"end_turn"}),
        json!({"kind":"text_delta","text":"hello","stderr":"private"}),
        json!({"kind":"turn_completed","resumable":false,"text":"hello","stopReason":"end_turn","receipt":"private"}),
        json!({"kind":"session_started","sessionId":"","resumable":true}),
        json!({"kind":"made_up_token","text":"hello"}),
        json!([{"kind":"text_delta","text":"hello"}]),
    ] {
        assert_eq!(
            decode_turn_output(&serde_json::to_vec(&value).unwrap()),
            Err(AgentError::ProtocolInvalid)
        );
    }
    assert_eq!(
        decode_turn_output(&vec![b' '; MAX_TURN_OUTPUT_BYTES + 1]),
        Err(AgentError::TurnOutputLimit)
    );
}

#[test]
fn metadata_report_is_readable_without_private_implementation() {
    let value = json!({"schemaVersion":1,"kind":"agent_connection_scan","readiness":"not_checked","connections":[{"id":"codex","adapter":"codex_cli","status":"detected","version":"0.160.1","protocolVersion":null,"sessionMode":"resumable","supportedInputFormats":["image/png","image/jpeg"],"error":null},{"id":"missing","adapter":"deepseek_harness","status":"unavailable","version":null,"protocolVersion":null,"sessionMode":null,"supportedInputFormats":[],"error":"executable_unavailable"}]});
    let report: ScanReport = serde_json::from_value(value.clone()).unwrap();
    report.validate().unwrap();
    assert_eq!(serde_json::to_value(&report).unwrap(), value);
    let mut invalid = report.clone();
    invalid.connections.push(report.connections[0].clone());
    assert_eq!(invalid.validate(), Err(AgentError::ProtocolInvalid));
    invalid = report;
    invalid.readiness = "model_available".into();
    assert_eq!(invalid.validate(), Err(AgentError::ProtocolInvalid));
}

#[test]
fn 型号和配置结果只表达中性有界声明() {
    let mut input = request();
    input["model"] = json!("relay/model-v2");
    assert_eq!(
        decode_turn_request(&serde_json::to_vec(&input).unwrap())
            .unwrap()
            .model
            .as_deref(),
        Some("relay/model-v2")
    );
    input["model"] = json!("--model injected");
    assert_eq!(
        decode_turn_request(&serde_json::to_vec(&input).unwrap()),
        Err(AgentError::RequestInvalid)
    );
    let metadata = json!({"provider":"relay","configuredModel":"relay/current","supportedModels":["relay/current"],"credentialEnvironmentVariables":["RELAY_API_KEY"],"configurationFingerprint":"a".repeat(64)});
    let parsed: ModelConfiguration = serde_json::from_value(metadata.clone()).unwrap();
    parsed.validate().unwrap();
    let mut invalid = metadata;
    invalid["credentialValues"] = json!({"RELAY_API_KEY":"fake"});
    assert!(serde_json::from_value::<ModelConfiguration>(invalid).is_err());
    let mut report = ConfigurationInspection {
        schema_version: 1,
        kind: "agent_configuration_inspection".into(),
        readiness: "not_checked".into(),
        agents: vec![InspectedAgent {
            id: "fixture".into(),
            adapter: Adapter::CodexCli,
            inspection_context_fingerprint: Some("b".repeat(64)),
            metadata: Some(parsed),
            prepared_model_override: None,
            error: None,
        }],
    };
    report.validate().unwrap();
    assert_eq!(
        serde_json::from_slice::<ConfigurationInspection>(&serde_json::to_vec(&report).unwrap())
            .unwrap(),
        report
    );
    report.agents[0].inspection_context_fingerprint = None;
    assert_eq!(report.validate(), Err(AgentError::ProtocolInvalid));
    report.agents[0].metadata = None;
    report.agents[0].error = Some(AgentError::ExecutableUnavailable);
    report.validate().unwrap();
    report.agents[0].inspection_context_fingerprint = Some("A".repeat(64));
    assert_eq!(report.validate(), Err(AgentError::ProtocolInvalid));
}

#[test]
fn 检查上下文指纹绑定适配器制品与规范目录且不读取文件() {
    use iquji_agent_wire::inspection_context_fingerprint;

    let digest = "a".repeat(64);
    let fingerprint =
        inspection_context_fingerprint(Adapter::CodexCli, &digest, "/tmp/agent-config", None)
            .unwrap();
    assert_eq!(
        fingerprint,
        "4e289e43dc9ca0b7787761579578a9e26569bcfd7146f41dc9c8b56aea11ded0"
    );
    for (adapter, digest, directory) in [
        (
            Adapter::DeepseekHarness,
            digest.clone(),
            "/tmp/agent-config",
        ),
        (Adapter::CodexCli, "b".repeat(64), "/tmp/agent-config"),
        (Adapter::CodexCli, digest.clone(), "/tmp/other-config"),
    ] {
        assert_ne!(
            inspection_context_fingerprint(
                adapter,
                &digest,
                directory,
                if adapter == Adapter::DeepseekHarness {
                    Some("/tmp/workspace")
                } else {
                    None
                }
            )
            .unwrap(),
            fingerprint
        );
    }
    for directory in [
        "",
        "relative",
        "/tmp/../config",
        "/tmp/./config",
        "/tmp//config",
        "/tmp/config/",
        "/tmp/control\n",
    ] {
        assert_eq!(
            inspection_context_fingerprint(Adapter::CodexCli, &digest, directory, None),
            Err(AgentError::ProtocolInvalid)
        );
    }
    for digest in [
        "a".repeat(63),
        "A".repeat(64),
        format!("sha256:{}", "a".repeat(64)),
    ] {
        assert_eq!(
            inspection_context_fingerprint(Adapter::CodexCli, &digest, "/tmp/agent-config", None),
            Err(AgentError::ProtocolInvalid)
        );
    }
}

#[test]
fn 覆盖描述只包含型号和有界文件摘要且工作区参与dsh身份() {
    let value =
        json!({"model":"relay/next","patch":{"size":2,"sha256":"a".repeat(64)},"settings":null});
    let prepared: PreparedModelOverride = serde_json::from_value(value.clone()).unwrap();
    prepared.validate().unwrap();
    assert_eq!(serde_json::to_value(prepared).unwrap(), value);
    for (field, replacement) in [
        ("size", json!(0)),
        ("size", json!(1_048_577)),
        ("sha256", json!("bad")),
    ] {
        let mut invalid = value.clone();
        invalid["patch"][field] = replacement;
        assert_eq!(
            serde_json::from_value::<PreparedModelOverride>(invalid)
                .unwrap()
                .validate(),
            Err(AgentError::ProtocolInvalid)
        );
    }
    let mut invalid = value;
    invalid["patch"]["path"] = json!("/private/not-a-returned-path");
    assert!(serde_json::from_value::<PreparedModelOverride>(invalid).is_err());
    assert_eq!(
        inspection_context_fingerprint(
            Adapter::DeepseekHarness,
            &"a".repeat(64),
            "/tmp/config",
            None
        ),
        Err(AgentError::ProtocolInvalid)
    );
    assert_ne!(
        inspection_context_fingerprint(
            Adapter::DeepseekHarness,
            &"a".repeat(64),
            "/tmp/config",
            Some("/tmp/workspace-one")
        )
        .unwrap(),
        inspection_context_fingerprint(
            Adapter::DeepseekHarness,
            &"a".repeat(64),
            "/tmp/config",
            Some("/tmp/workspace-two")
        )
        .unwrap()
    );
}

#[test]
fn 回合上下文绑定制品工作区和配置根() {
    let digest = "a".repeat(64);
    let base = turn_context_fingerprint(
        Adapter::DeepseekHarness,
        &digest,
        "/workspace",
        Some("/config"),
    )
    .unwrap();
    for changed in [
        turn_context_fingerprint(Adapter::CodexCli, &digest, "/workspace", Some("/config")),
        turn_context_fingerprint(
            Adapter::DeepseekHarness,
            &"b".repeat(64),
            "/workspace",
            Some("/config"),
        ),
        turn_context_fingerprint(Adapter::DeepseekHarness, &digest, "/other", Some("/config")),
        turn_context_fingerprint(
            Adapter::DeepseekHarness,
            &digest,
            "/workspace",
            Some("/other"),
        ),
    ] {
        assert_ne!(base, changed.unwrap());
    }
    assert!(
        turn_context_fingerprint(Adapter::DeepseekHarness, &digest, "/workspace", None).is_err()
    );
    assert!(
        turn_context_fingerprint(Adapter::NativeAcp, &digest, "/workspace", Some("/config"))
            .is_err()
    );
    assert!(turn_context_fingerprint(Adapter::NativeAcp, &digest, "/workspace", None).is_ok());
    assert!(
        turn_context_fingerprint(
            Adapter::DeepseekHarness,
            &digest,
            "/workspace/../other",
            Some("/config")
        )
        .is_err()
    );
    let mut value = request();
    value["contextFingerprint"] = json!("invalid");
    assert!(decode_turn_request(&serde_json::to_vec(&value).unwrap()).is_err());
}

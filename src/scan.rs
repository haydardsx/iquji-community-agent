use crate::{Adapter, Agent, AgentError, Configuration, Connection, ScanReport, version::Version};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

pub fn scan(configuration: &Configuration, cancelled: &AtomicBool) -> ScanReport {
    scan_inner(configuration, cancelled, false)
}

pub fn scan_with_context(configuration: &Configuration, cancelled: &AtomicBool) -> ScanReport {
    scan_inner(configuration, cancelled, true)
}

fn scan_inner(configuration: &Configuration, cancelled: &AtomicBool, bound: bool) -> ScanReport {
    let connections = configuration
        .agents
        .iter()
        .map(|agent| {
            let mut result = Connection {
                id: agent.id.clone(),
                adapter: agent.adapter,
                context_fingerprint: None,
                executable_sha256: None,
                status: "unavailable".into(),
                version: None,
                protocol_version: None,
                session_mode: None,
                supported_input_formats: Vec::new(),
                error: None,
            };
            let deadline = std::time::Instant::now()
                + std::time::Duration::from_millis(configuration.timeout_ms);
            let outcome = (|| {
                let context = if bound {
                    Some(crate::configuration_inspection::turn_context(
                        agent,
                        configuration,
                        cancelled,
                        deadline,
                    )?)
                } else {
                    None
                };
                let detected = inspect(agent, configuration, cancelled, deadline)?;
                if let Some(expected) = &context
                    && crate::configuration_inspection::turn_context(
                        agent,
                        configuration,
                        cancelled,
                        deadline,
                    )? != *expected
                {
                    return Err(AgentError::ExecutableDigestMismatch);
                }
                Ok((detected, context))
            })();
            match outcome {
                Ok((detected, context)) => {
                    result.status = "detected".into();
                    result.context_fingerprint = context.as_ref().map(|value| value.0.clone());
                    result.executable_sha256 = context.map(|value| value.1);
                    result.version = detected.version.map(|value| value.to_string());
                    result.protocol_version = detected.protocol_version;
                    result.session_mode = Some(detected.mode.into());
                    if detected.images {
                        result.supported_input_formats =
                            vec!["image/png".into(), "image/jpeg".into()];
                    }
                }
                Err(error) => result.error = Some(error),
            }
            result
        })
        .collect();
    ScanReport {
        schema_version: 1,
        kind: "agent_connection_scan".into(),
        readiness: "not_checked".into(),
        connections,
    }
}

struct Detection {
    version: Option<Version>,
    mode: &'static str,
    images: bool,
    protocol_version: Option<u16>,
}

fn inspect(
    agent: &Agent,
    configuration: &Configuration,
    cancelled: &AtomicBool,
    deadline: std::time::Instant,
) -> Result<Detection, AgentError> {
    let executable = crate::configuration_inspection::turn_executable(agent, cancelled, deadline)?;
    if matches!(
        agent.adapter,
        Adapter::NativeAcp | Adapter::ExternalAcpAdapter
    ) {
        let mut remaining = configuration.clone();
        remaining.timeout_ms = deadline
            .saturating_duration_since(std::time::Instant::now())
            .as_millis()
            .min(u64::MAX as u128) as u64;
        if remaining.timeout_ms == 0 {
            return Err(AgentError::ProbeTimedOut);
        }
        let capabilities = crate::acp::inspect(&executable, &remaining, cancelled)?;
        return Ok(Detection {
            version: None,
            mode: if capabilities.load_session {
                "resumable"
            } else {
                "connection_scoped"
            },
            images: capabilities.images,
            protocol_version: Some(1),
        });
    }
    let home = crate::cli_environment::CliEnvironment::isolated(agent.adapter)?;
    #[cfg(unix)]
    let mut budget = crate::process_io::ProcessBudget {
        deadline,
        output_left: configuration.max_output_bytes,
    };
    let invoke = |arguments: &[&str]| {
        #[cfg(unix)]
        {
            crate::probe::probe_with_environment(
                &executable,
                arguments,
                configuration,
                cancelled,
                &mut budget,
                &home.probe_values(),
            )
        }
        #[cfg(not(unix))]
        {
            let _ = (&home, arguments);
            Err(AgentError::PlatformUnsupported)
        }
    };
    let (version, mode, images) = match agent.adapter {
        Adapter::CodexCli => {
            let (version, images, _) = codex_capabilities(invoke)?;
            (version, "resumable", images)
        }
        Adapter::DeepseekHarness => (deepseek_capabilities(invoke)?.0, "single_turn", false),
        _ => return Err(AgentError::ProtocolInvalid),
    };
    Ok(Detection {
        version: Some(version),
        mode,
        images,
        protocol_version: None,
    })
}

pub(crate) fn deepseek_capabilities(
    mut invoke: impl FnMut(&[&str]) -> Result<String, AgentError>,
) -> Result<(Version, bool), AgentError> {
    let version = Version::parse(&invoke(&["--version"])?).ok_or(AgentError::VersionUnsupported)?;
    if version < Version(0, 1, 0) {
        return Err(AgentError::VersionUnsupported);
    }
    let help = invoke(&["--help"])?;
    if !has_token(&help, "--profile") {
        return Err(AgentError::CapabilitiesUnsupported);
    }
    Ok((
        version,
        has_token(&help, "--dump-config") && has_token(&help, "--patch"),
    ))
}

pub(crate) fn codex_capabilities(
    mut invoke: impl FnMut(&[&str]) -> Result<String, AgentError>,
) -> Result<(Version, bool, bool), AgentError> {
    let version = Version::parse(&invoke(&["--version"])?).ok_or(AgentError::VersionUnsupported)?;
    if version < Version(0, 20, 0) {
        return Err(AgentError::VersionUnsupported);
    }
    let help = invoke(&["exec", "--help"])?;
    if ![
        "--json",
        "--color",
        "--sandbox",
        "--skip-git-repo-check",
        "-C",
        "resume",
    ]
    .iter()
    .all(|marker| has_token(&help, marker))
    {
        return Err(AgentError::CapabilitiesUnsupported);
    }
    let resume = invoke(&["exec", "resume", "--help"])?;
    if resume.trim().is_empty() {
        return Err(AgentError::CapabilitiesUnsupported);
    }
    Ok((
        version,
        has_token(&help, "--image") && has_token(&resume, "--image"),
        has_token(&help, "--model") || has_token(&help, "-m"),
    ))
}

fn has_token(help: &str, option: &str) -> bool {
    help.split_whitespace()
        .any(|token| token.trim_end_matches(',') == option)
}

pub(crate) fn executable(agent: &Agent) -> Result<PathBuf, AgentError> {
    let program =
        std::fs::canonicalize(&agent.executable).map_err(|_| AgentError::ExecutableUnavailable)?;
    let metadata =
        std::fs::symlink_metadata(&program).map_err(|_| AgentError::ExecutableUnavailable)?;
    if !metadata.is_file() {
        return Err(AgentError::ExecutableNotRegular);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(AgentError::ExecutableNotExecutable);
        }
    }
    Ok(program)
}

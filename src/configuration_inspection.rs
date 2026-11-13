use crate::{Adapter, AgentError, Configuration};
use iquji_agent_wire::{
    ConfigurationInspection, InspectedAgent, ModelConfiguration, PreparedModelOverride,
    inspection_context_fingerprint,
};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

const MAX_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;
type ExecutableIdentity = (u64, u64, u64, u32, i64, i64, i64, i64);

pub(crate) fn turn_context(
    agent: &crate::Agent,
    configuration: &Configuration,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<(String, String), AgentError> {
    let program = crate::scan::executable(agent)?;
    let (digest, _) = executable_digest(&program, cancelled, deadline)?;
    if agent
        .expected_sha256
        .as_ref()
        .is_some_and(|expected| *expected != digest)
    {
        return Err(AgentError::ExecutableDigestMismatch);
    }
    let directory = match agent.adapter {
        Adapter::CodexCli | Adapter::DeepseekHarness => Some(configuration_directory(agent)?),
        _ => None,
    };
    let context = iquji_agent_wire::turn_context_fingerprint(
        agent.adapter,
        &digest,
        configuration
            .working_directory
            .to_str()
            .ok_or(AgentError::ConfigurationInvalid)?,
        directory
            .map(|path| path.to_str().ok_or(AgentError::ConfigurationInvalid))
            .transpose()?,
    )?;
    Ok((context, digest))
}

pub fn inspect_configuration(
    configuration: &Configuration,
    cancelled: &AtomicBool,
) -> ConfigurationInspection {
    inspect_configuration_with_model(configuration, cancelled, None)
}

pub fn inspect_configuration_with_model(
    configuration: &Configuration,
    cancelled: &AtomicBool,
    preparation: Option<(&str, &Path)>,
) -> ConfigurationInspection {
    let agents = configuration
        .agents
        .iter()
        .map(|agent| {
            let outcome = inspect(agent, configuration, cancelled, preparation).and_then(
                |(value, context, prepared)| {
                    value
                        .validate()
                        .map_err(|_| AgentError::ConfigurationMetadataInvalid)?;
                    Ok((value, context, prepared))
                },
            );
            let (metadata, inspection_context_fingerprint, prepared_model_override, error) =
                match outcome {
                    Ok((value, context, prepared)) => (Some(value), Some(context), prepared, None),
                    Err(error) => (None, None, None, Some(error)),
                };
            InspectedAgent {
                id: agent.id.clone(),
                adapter: agent.adapter,
                inspection_context_fingerprint,
                metadata,
                prepared_model_override,
                error,
            }
        })
        .collect();
    ConfigurationInspection {
        schema_version: 1,
        kind: "agent_configuration_inspection".into(),
        readiness: "not_checked".into(),
        agents,
    }
}

fn inspect(
    agent: &crate::Agent,
    configuration: &Configuration,
    cancelled: &AtomicBool,
    preparation: Option<(&str, &Path)>,
) -> Result<(ModelConfiguration, String, Option<PreparedModelOverride>), AgentError> {
    let deadline = Instant::now() + std::time::Duration::from_millis(configuration.timeout_ms);
    let (program, context, identity) =
        inspection_context(agent, configuration, cancelled, deadline)?;
    if preparation.is_some()
        && (configuration.agents.len() != 1 || agent.adapter != Adapter::DeepseekHarness)
    {
        return Err(AgentError::ModelUnsupported);
    }
    #[cfg(unix)]
    if let Some((_, directory)) = preparation {
        crate::model_artifact::validate_directory(directory)?;
    }
    let (metadata, prepared) = inspect_metadata(
        agent,
        configuration,
        cancelled,
        deadline,
        &program,
        preparation,
    )?;
    configuration_directory(agent)?;
    let after =
        std::fs::symlink_metadata(&program).map_err(|_| AgentError::ExecutableUnavailable)?;
    if std::fs::canonicalize(&agent.executable).ok().as_ref() != Some(&program)
        || executable_identity(&after) != identity
    {
        return Err(AgentError::ExecutableDigestMismatch);
    }
    crate::configuration_io::check(cancelled, deadline)?;
    Ok((metadata, context, prepared))
}

fn inspect_metadata(
    agent: &crate::Agent,
    configuration: &Configuration,
    cancelled: &AtomicBool,
    deadline: Instant,
    program: &Path,
    preparation: Option<(&str, &Path)>,
) -> Result<(ModelConfiguration, Option<PreparedModelOverride>), AgentError> {
    match agent.adapter {
        Adapter::CodexCli => {
            crate::model_codex::inspect(agent, cancelled, deadline).map(|metadata| (metadata, None))
        }
        #[cfg(unix)]
        Adapter::DeepseekHarness => {
            let home = crate::cli_environment::CliEnvironment::new(agent)?;
            let isolated = crate::cli_environment::CliEnvironment::isolated(agent.adapter)?;
            let environment = home.inspection_values(&isolated);
            let mut budget = crate::process_io::ProcessBudget {
                deadline,
                output_left: crate::configuration_io::MAX_CONFIGURATION_BYTES,
            };
            let (_, supported) = crate::scan::deepseek_capabilities(|arguments| {
                crate::probe::probe_with_environment(
                    program,
                    arguments,
                    configuration,
                    cancelled,
                    &mut budget,
                    &isolated.probe_values(),
                )
            })?;
            if !supported {
                return Err(AgentError::ModelUnsupported);
            }
            let parsed = crate::model_deepseek::inspect(
                agent,
                configuration,
                cancelled,
                &mut budget,
                &environment,
            )?;
            let metadata = parsed.metadata.clone();
            let prepared = match preparation {
                Some((model, directory)) => Some(crate::model_artifact::prepare(
                    parsed, model, directory, cancelled, deadline,
                )?),
                None => None,
            };
            Ok((metadata, prepared))
        }
        _ => Err(AgentError::ModelUnsupported),
    }
}

fn inspection_context(
    agent: &crate::Agent,
    configuration: &Configuration,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<(std::path::PathBuf, String, ExecutableIdentity), AgentError> {
    crate::configuration_io::check(cancelled, deadline)?;
    let directory = configuration_directory(agent)?;
    let program = crate::scan::executable(agent)?;
    let (digest, identity) = executable_digest(&program, cancelled, deadline)?;
    if agent
        .expected_sha256
        .as_ref()
        .is_some_and(|expected| *expected != digest)
    {
        return Err(AgentError::ExecutableDigestMismatch);
    }
    let fingerprint = inspection_context_fingerprint(
        agent.adapter,
        &digest,
        directory
            .to_str()
            .ok_or(AgentError::ConfigurationMetadataInvalid)?,
        if agent.adapter == Adapter::DeepseekHarness {
            configuration.working_directory.to_str()
        } else {
            None
        },
    )?;
    Ok((program, fingerprint, identity))
}

fn configuration_directory(agent: &crate::Agent) -> Result<&Path, AgentError> {
    let directory = match agent.adapter {
        Adapter::CodexCli => agent.codex_configuration_directory.as_ref(),
        Adapter::DeepseekHarness => agent.deepseek_configuration_directory.as_ref(),
        _ => return Err(AgentError::ModelUnsupported),
    }
    .ok_or(AgentError::ConfigurationMetadataUnavailable)?;
    if std::fs::canonicalize(directory).ok().as_ref() != Some(directory) || !directory.is_dir() {
        return Err(AgentError::ConfigurationMetadataUnavailable);
    }
    Ok(directory)
}

fn executable_digest(
    path: &Path,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<(String, ExecutableIdentity), AgentError> {
    let before = std::fs::symlink_metadata(path).map_err(|_| AgentError::ExecutableUnavailable)?;
    validate_executable(&before)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options
        .open(path)
        .map_err(|_| AgentError::ExecutableUnavailable)?;
    let opened = file
        .metadata()
        .map_err(|_| AgentError::ExecutableUnavailable)?;
    validate_executable(&opened)?;
    if executable_identity(&before) != executable_identity(&opened) {
        return Err(AgentError::ExecutableDigestMismatch);
    }
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 8192];
    loop {
        crate::configuration_io::check(cancelled, deadline)?;
        let count = file
            .read(&mut buffer)
            .map_err(|_| AgentError::ExecutableUnavailable)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_EXECUTABLE_BYTES {
            return Err(AgentError::ExecutableUnavailable);
        }
        digest.update(&buffer[..count]);
    }
    let after = std::fs::symlink_metadata(path).map_err(|_| AgentError::ExecutableUnavailable)?;
    let opened = file
        .metadata()
        .map_err(|_| AgentError::ExecutableUnavailable)?;
    if std::fs::canonicalize(path).ok().as_deref() != Some(path)
        || executable_identity(&before) != executable_identity(&after)
        || executable_identity(&before) != executable_identity(&opened)
    {
        return Err(AgentError::ExecutableDigestMismatch);
    }
    crate::configuration_io::check(cancelled, deadline)?;
    Ok((
        format!("{:x}", digest.finalize()),
        executable_identity(&before),
    ))
}

fn validate_executable(metadata: &std::fs::Metadata) -> Result<(), AgentError> {
    if !metadata.is_file() {
        return Err(AgentError::ExecutableNotRegular);
    }
    if metadata.len() > MAX_EXECUTABLE_BYTES {
        return Err(AgentError::ExecutableUnavailable);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(AgentError::ExecutableNotExecutable);
        }
    }
    Ok(())
}

fn executable_identity(metadata: &std::fs::Metadata) -> ExecutableIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mode(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    }
    #[cfg(not(unix))]
    {
        (0, 0, metadata.len(), 0, 0, 0, 0, 0)
    }
}

pub(crate) fn turn_executable(
    agent: &crate::Agent,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<std::path::PathBuf, AgentError> {
    let program = crate::scan::executable(agent)?;
    if let Some(expected) = &agent.expected_sha256
        && executable_digest(&program, cancelled, deadline)?.0 != *expected
    {
        return Err(AgentError::ExecutableDigestMismatch);
    }
    Ok(program)
}

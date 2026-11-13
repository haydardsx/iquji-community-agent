use crate::{Adapter, AgentError};

/// 宿主与工具分别核验本机事实后，使用同一摘要绑定本回合的执行上下文。
pub fn turn_context_fingerprint(
    adapter: Adapter,
    executable_sha256: &str,
    working_directory: &str,
    configuration_directory: Option<&str>,
) -> Result<String, AgentError> {
    use crate::configuration::{valid_directory, valid_fingerprint};
    use sha2::{Digest, Sha256};
    if !valid_fingerprint(executable_sha256)
        || !valid_directory(working_directory)
        || configuration_directory.is_some_and(|value| !valid_directory(value))
        || matches!(adapter, Adapter::CodexCli | Adapter::DeepseekHarness)
            != configuration_directory.is_some()
    {
        return Err(AgentError::ProtocolInvalid);
    }
    let adapter = serde_json::to_vec(&adapter).map_err(|_| AgentError::ProtocolInvalid)?;
    let mut digest = Sha256::new();
    for field in [
        b"iquji-agent-turn-context/v1".as_slice(),
        adapter.as_slice(),
        executable_sha256.as_bytes(),
        working_directory.as_bytes(),
        configuration_directory.unwrap_or_default().as_bytes(),
    ] {
        digest.update((field.len() as u32).to_be_bytes());
        digest.update(field);
    }
    Ok(format!("{:x}", digest.finalize()))
}

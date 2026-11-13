use crate::{AgentError, Configuration};
use std::path::Path;
use std::sync::atomic::AtomicBool;

#[cfg(unix)]
pub(crate) fn probe_with_environment(
    program: &Path,
    arguments: &[&str],
    configuration: &Configuration,
    cancelled: &AtomicBool,
    budget: &mut crate::process_io::ProcessBudget,
    environment: &std::collections::BTreeMap<String, std::ffi::OsString>,
) -> Result<String, AgentError> {
    let original_deadline = budget.deadline;
    budget.deadline = budget.deadline.min(
        std::time::Instant::now() + std::time::Duration::from_millis(configuration.timeout_ms),
    );
    let mut output = Vec::new();
    let result = crate::process_io::run(
        crate::process_io::Invocation {
            program,
            arguments,
            input: None,
            environment,
            output_limit: configuration.max_output_bytes,
        },
        configuration,
        cancelled,
        budget,
        |bytes| {
            output.extend_from_slice(bytes);
            Ok(())
        },
    );
    budget.deadline = original_deadline;
    result?;
    String::from_utf8(output).map_err(|_| AgentError::CapabilitiesUnsupported)
}

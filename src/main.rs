use iquji_agent_wire::{PROCESS_GROUP_OWNER_ARGUMENT, ProcessExecutionContext};
use iquji_shared_agent::{
    AgentError, Configuration, inspect_configuration_with_model, scan, scan_with_context,
};
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

mod cli_turn;

static CANCELLED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn cancel(_signal: i32) {
    CANCELLED.store(true, Ordering::SeqCst);
}

fn main() {
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGINT, cancel as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, cancel as *const () as libc::sighandler_t);
    }
    let mut arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let result = process_context(&mut arguments).and_then(|context| execute(&arguments, context));
    match result {
        Ok(()) => {}
        Err(error) => {
            if arguments.len() >= 3
                && matches!(arguments[2].to_str(), Some("turn" | "inspect" | "scan"))
            {
                cli_turn::diagnostic(error);
            } else {
                eprintln!("{}", error.code());
            }
            std::process::exit(2);
        }
    }
}

fn process_context(
    arguments: &mut Vec<OsString>,
) -> Result<Option<ProcessExecutionContext>, AgentError> {
    if arguments.len() < 2 || arguments[arguments.len() - 2] != PROCESS_GROUP_OWNER_ARGUMENT {
        return Ok(None);
    }
    let group = arguments.pop().ok_or(AgentError::ConfigurationInvalid)?;
    arguments.pop();
    let context = ProcessExecutionContext {
        owner_process_group: group
            .to_str()
            .and_then(|value| value.parse().ok())
            .ok_or(AgentError::ConfigurationInvalid)?,
    };
    context.validate()?;
    Ok(Some(context))
}

fn load_configuration(
    path: &Path,
    context: Option<ProcessExecutionContext>,
) -> Result<Configuration, AgentError> {
    let configuration = Configuration::load(path)?;
    match context {
        Some(context) => configuration.with_process_context(context),
        None => Ok(configuration),
    }
}

fn execute(
    arguments: &[OsString],
    context: Option<ProcessExecutionContext>,
) -> Result<(), AgentError> {
    if matches!(arguments.len(), 3 | 4) && arguments[0] == "--config" && arguments[2] == "scan" {
        load_configuration(Path::new(&arguments[1]), context).and_then(|mut configuration| {
            let started = std::time::Instant::now();
            let report = if let Some(id) = arguments.get(3) {
                configuration.select_agent(id.to_str().ok_or(AgentError::RequestInvalid)?)?;
                scan_with_context(&configuration, &CANCELLED)
            } else {
                scan(&configuration, &CANCELLED)
            };
            cli_turn::metadata_report(&report, &configuration, &CANCELLED, started)
        })
    } else if matches!(arguments.len(), 3 | 4 | 8)
        && arguments[0] == "--config"
        && arguments[2] == "inspect"
    {
        load_configuration(Path::new(&arguments[1]), context).and_then(|mut configuration| {
            if let Some(id) = arguments.get(3) {
                configuration.select_agent(id.to_str().ok_or(AgentError::RequestInvalid)?)?;
            }
            let preparation = if arguments.len() == 8 {
                if arguments[4] != "--model" || arguments[6] != "--output-directory" {
                    return Err(AgentError::RequestInvalid);
                }
                let model = arguments[5]
                    .to_str()
                    .filter(|model| iquji_agent_wire::valid_model_identifier(model))
                    .ok_or(AgentError::RequestInvalid)?;
                Some((model, Path::new(&arguments[7])))
            } else {
                None
            };
            let started = std::time::Instant::now();
            let report = inspect_configuration_with_model(&configuration, &CANCELLED, preparation);
            cli_turn::metadata_report(&report, &configuration, &CANCELLED, started)
        })
    } else if arguments.len() == 3 && arguments[0] == "--config" && arguments[2] == "turn" {
        match load_configuration(Path::new(&arguments[1]), context) {
            Ok(configuration) => cli_turn::run(&configuration, &CANCELLED),
            Err(error) => {
                cli_turn::configuration_error(error);
                Err(error)
            }
        }
    } else {
        Err(AgentError::ConfigurationInvalid)
    }
}

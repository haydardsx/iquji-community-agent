//! 本地代码代理的能力发现、回合执行与进程监督。

mod acp;
#[cfg(unix)]
mod acp_transport;
mod attachments;
mod cli_codex;
mod cli_deepseek;
mod cli_environment;
#[cfg(unix)]
mod codex_mcp;
mod config;
mod configuration_inspection;
mod configuration_io;
#[cfg(unix)]
mod model_artifact;
mod model_codex;
#[cfg(unix)]
mod model_deepseek;
mod model_files;
mod private_directory;
mod probe;
mod process;
#[cfg(unix)]
mod process_io;
mod scan;
mod turn;
mod version;

pub use config::{Agent, Configuration};
pub use configuration_inspection::{inspect_configuration, inspect_configuration_with_model};
pub use iquji_agent_wire::{
    Adapter, AgentError, Attachment, Connection, ProcessExecutionContext, ScanReport, TurnEvent,
    TurnOutput, TurnRequest, TurnResult, TurnTerminal,
};
pub use scan::{scan, scan_with_context};
pub use turn::run_turn;

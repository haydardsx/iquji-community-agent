use iquji_agent_wire::{
    AgentError, MAX_REQUEST_BYTES, TurnResult, TurnTerminal, decode_turn_request,
};
use std::io::Read;

fn main() {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .unwrap();
    let result = decode_turn_request(&bytes).and_then(|request| {
        if request.session_id.is_some() {
            return Err(AgentError::SessionUnsupported);
        }
        Ok(TurnResult {
            session_id: None,
            resumable: false,
            text: request.prompt,
            stop_reason: "end_turn".into(),
        })
    });
    let failed = result.is_err();
    let terminal = match result {
        Ok(result) => TurnTerminal::TurnCompleted { result },
        Err(error) => TurnTerminal::TurnError { error },
    };
    println!("{}", serde_json::to_string(&terminal).unwrap());
    if failed {
        std::process::exit(2);
    }
}

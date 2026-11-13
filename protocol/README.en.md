# iquji-agent-wire

<div align="center">

[![Crates.io: iquji-agent-wire](https://img.shields.io/crates/v/iquji-agent-wire.svg?label=iquji-agent-wire&color=teal)](https://crates.io/crates/iquji-agent-wire)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](../LICENSE)
[![Rust: 1.97.1+](https://img.shields.io/badge/Rust-1.97.1%2B%20(2024)-orange.svg)](../rust-toolchain.toml)
[![Docs.rs](https://img.shields.io/badge/docs.rs-iquji--agent--wire-purple.svg)](https://docs.rs/iquji-agent-wire)

**Agent Machine Contract and Data Protocol Layer (Rust / Serde)**

[简体中文](README.md) | [English](README.en.md)

</div>

---

## Overview

`iquji-agent-wire` is the lightweight machine contract and data protocol crate within the `iquji-agent` ecosystem. It defines the Rust/Serde data models and pure validation functions for Agent turn requests (`TurnRequest`), streaming events (`TurnEvent`), terminal states (`TurnTerminal`), scan reports (`ScanReport`), and configuration inspection metadata (`ConfigurationInspection`).

Core architectural principles:
- **Pure Data & Zero Heavy Dependencies**: Depends only on pinned versions of Serde, JSON codecs, SHA-256, and URL parsers. It introduces no subprocess spawning, network I/O, or OS-specific supervisor logic.
- **Strict Invariant Validation**: Enforces rigorous deserialization guards, rejecting unexpected fields, trailing JSON tokens, control characters, and invalid state combinations.
- **Reproducible Security Fingerprinting**: Exposes side-effect-free context hashing functions (`turn_context_fingerprint` and `inspection_context_fingerprint`).
- **Built-in Compliance Fixture**: Includes a reference test host fixture in `tests/fixtures/host` to verify end-to-end wire protocol compliance.

---

## Installation & Consumption

Add the lightweight crate dependency via crates.io:

```toml
[dependencies]
iquji-agent-wire = "0.1.0"
```

For workspace development within this repository, use the relative path dependency:

```toml
[dependencies]
iquji-agent-wire = { version = "0.1.0", path = "protocol" }
```

---

## Core Types and Interfaces

| Type / Function | Description & Validation Rules |
| --- | --- |
| `TurnRequest` | Single-turn request containing `agentId`, `prompt` (≤64KB), optional `sessionId` (≤512B), `model`, `attachments`, and up to 32 `mcpServers` |
| `TurnOutput` | Untagged union representing an NDJSON stream line: either a `TurnEvent` or a `TurnTerminal` |
| `TurnEvent` | Stream event: `SessionStarted { session_id, resumable }` or `TextDelta { text }` |
| `TurnTerminal` | Terminal result: `TurnCompleted { result }` or `TurnError { error }` |
| `AgentError` | Stable error code enumeration covering configuration, executable, timeout, quota, session, and protocol failures |
| `ScanReport` | Local agent scan report containing connection statuses and capability descriptors |
| `ConfigurationInspection` | Safe configuration inspection report (Provider, models, and credential variable names/fingerprints) |
| `ProcessExecutionContext` | Process supervision context descriptor (`ownerProcessGroup`), rejecting 0, 1, or out-of-range PIDs |
| `decode_turn_request` | Strictly decodes a JSON byte slice into a `TurnRequest`, rejecting trailing bytes and unknown fields |
| `decode_turn_output` | Parses a single NDJSON line into a `TurnOutput` with full validation |
| `turn_context_fingerprint` | Computes a 64-character SHA-256 bound digest of the adapter, executable hash, working directory, and config path |

---

## Code Examples

### 1. Decoding and Validating Streaming NDJSON Frames
```rust
use iquji_agent_wire::{TurnOutput, decode_turn_output};

fn process_stream_frame(line: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let output = decode_turn_output(line)?;
    match output {
        TurnOutput::Event(event) => {
            println!("Received stream event: {event:?}");
        }
        TurnOutput::Terminal(terminal) => {
            println!("Turn terminated: {terminal:?}");
        }
    }
    Ok(())
}
```

### 2. Computing an Irreversible Context Fingerprint
```rust
use iquji_agent_wire::{Adapter, turn_context_fingerprint};
use std::path::Path;

let fingerprint = turn_context_fingerprint(
    Adapter::CodexCli,
    Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
    Path::new("/canonical/workspace"),
    Some(Path::new("/canonical/codex-config")),
);
println!("Context Fingerprint: {fingerprint}");
```

---

## Protocol Constraints & Security Guarantees

1. **MCP Server Constraints**:
   - At most 32 uniquely named MCP server entries.
   - Every URL must strictly match `http://127.0.0.1:<port>/mcp/<48-char-hex-token>` on loopback.
   - Usernames, passwords, queries, and URL fragments are strictly rejected.
2. **Workspace Write Access (`workspaceWrite`)**:
   - Defaults strictly to `false` (read-only).
   - Only set to `true` when explicit host user authorization has occurred. The wire contract provides no paths; the supervisor confines writes to the configured working directory.
3. **Privacy & Irreversibility**:
   - Fingerprints, inspection reports, and model overrides transmit only file sizes and cryptographic digests, never raw file contents, tokens, or credentials.

---

## Testing & Compliance Verification

Run the test suite and linters within the `protocol` directory:

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

The test suite includes a standalone fixture host (`tests/fixtures/host`), which compiles in an isolated temporary directory to verify bidirectional standard I/O machine contracts.

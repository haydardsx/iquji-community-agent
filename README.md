# IQuji Community Agent

[简体中文](README.zh-CN.md)

Connect to locally configured coding agents from Rust or the command line. The library discovers configured agents, inspects their capabilities and streams turn events. The CLI reads a JSON configuration.

```sh
cargo install iquji-community-agent --version 0.2.0 --locked
```

Create `agents.json` from [the example](config.example.json) and set absolute paths for the workspace and the agent executable. On macOS or Linux, restrict access before scanning.

```sh
chmod 600 agents.json
iquji-shared-agent --config "$PWD/agents.json" scan
```

To integrate the wire protocol without starting an agent, use [iquji-community-agent-wire](protocol/README.md).

Build and test with Rust 1.97.1 using `cargo test --workspace --locked`. Released under the [MIT License](LICENSE).

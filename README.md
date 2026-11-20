# IQuji Community Agent

[简体中文](README.zh-CN.md)

Connect to locally configured coding agents from Rust or the command line. The library discovers configured agents, inspects their capabilities and streams turn events. The CLI reads a JSON configuration.

```sh
cargo install iquji-community-agent --version 0.2.0 --locked
iquji-shared-agent --config agents.json scan
```

Start with [config.example.json](config.example.json). Set the workspace and executable paths to the locations on your machine. To integrate the wire protocol without starting an agent, use [iquji-community-agent-wire](protocol/README.md).

Build and test with Rust 1.97.1 using `cargo test --workspace --locked`. Released under the [MIT License](LICENSE).

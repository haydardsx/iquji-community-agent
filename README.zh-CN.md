# IQuji Community Agent

[English](README.md)

通过 Rust 或命令行连接本机配置的代码代理。库可以发现已配置的代理、查询其能力并读取流式事件。命令行工具使用 JSON 配置。

```sh
cargo install iquji-community-agent --version 0.2.0 --locked
iquji-shared-agent --config agents.json scan
```

参考[示例配置](config.example.json)，将工作目录和可执行文件路径替换为本机实际位置。只需读写协议消息时，可使用 [iquji-community-agent-wire](protocol/README.zh-CN.md)。

使用 Rust 1.97.1 运行 `cargo test --workspace --locked`。本项目以 [MIT 许可证](LICENSE)发布。

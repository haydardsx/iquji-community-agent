# IQuji Community Agent

[English](README.md)

通过 Rust 或命令行连接本机配置的代码代理。库可以发现已配置的代理、查询其能力并读取流式事件。命令行工具使用 JSON 配置。

```sh
cargo install iquji-community-agent --version 0.2.0 --locked
```

参考[示例配置](config.example.json)创建 `agents.json`，填写工作目录和代理可执行文件的绝对路径。在 macOS 或 Linux 上，先限制配置文件的访问权限，再扫描代理。

```sh
chmod 600 agents.json
iquji-shared-agent --config "$PWD/agents.json" scan
```

只需读写协议消息时，可使用 [iquji-community-agent-wire](protocol/README.zh-CN.md)。

使用 Rust 1.97.1 运行 `cargo test --workspace --locked`。本项目以 [MIT 许可证](LICENSE)发布。

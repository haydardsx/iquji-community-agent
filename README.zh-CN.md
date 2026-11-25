# IQuji Community Agent

[English](README.md)

IQuji Community Agent 提供 Rust 库和命令行工具，供开发者在应用或脚本中接入本机已配置的代码代理。可检查代理是否可用、查询能力并提交提示词，通过 Agent Client Protocol（ACP）、Codex CLI 或 DeepSeek Harness 适配器通信。若适配器支持流式输出，可逐段读取回复。

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

## 参与贡献

从最新的 `origin/main` 建立用途分支，例如 `feat/agent-discovery`。分支名由 Conventional Commits 类型、`/` 和主题组成。主题使用小写英文字母及数字，单词之间使用一个连字符，分支名总长至多 64 个字符。

```sh
python3 scripts/check_staged.py --install
```

暂存本次要提交的改动后执行提交。钩子会在临时检出中对暂存快照运行本仓 CI 命令，并在当前系统打包和检查两个 crate。GitHub Actions 在 Linux 上执行相同的制品检查。也可在提交前直接运行 `python3 scripts/check_staged.py`。已有钩子时，在 `pre-commit` 接入该命令，并在 `commit-msg` 接入 `python3 .github/scripts/check_subjects.py --commit-message "$1"`。

提交和拉取请求的标题使用 Conventional Commits 格式，例如 `feat(agent): improve agent discovery`。可用范围为 `agent`、`cli`、`wire`、`protocol`、`config`、`runtime`、`docs`、`deps`、`release`、`ci`、`test` 和 `build`。送审提交的作者和提交者使用正式维护身份 `IQuji Community <community@iqu.lol>`。向 `main` 提交拉取请求并等待必需检查通过。

# iquji-agent-wire

<div align="center">

[![Crates.io: iquji-agent-wire](https://img.shields.io/crates/v/iquji-agent-wire.svg?label=iquji-agent-wire&color=teal)](https://crates.io/crates/iquji-agent-wire)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](../LICENSE)
[![Rust: 1.97.1+](https://img.shields.io/badge/Rust-1.97.1%2B%20(2024)-orange.svg)](../rust-toolchain.toml)
[![Docs.rs](https://img.shields.io/badge/docs.rs-iquji--agent--wire-purple.svg)](https://docs.rs/iquji-agent-wire)

**Agent 进程机器合同与纯数据协议层 (Rust / Serde)**

[简体中文](README.md) | [English](README.en.md)

</div>

---

## 概述

`iquji-agent-wire` 是 `iquji-agent` 体系中的轻量级机器通信合同与数据协议库。它定义了通用的 Agent 请求 (`TurnRequest`)、事件 (`TurnEvent`)、终态 (`TurnTerminal`)、扫描报告 (`ScanReport`) 与配置元数据 (`ConfigurationInspection`) 的 Rust/Serde 类型与校验纯函数。

本 crate 的核心设计原则：
- **纯数据与零重量级依赖**：仅依赖精确锁定的 Serde、JSON 编解码器、SHA-256 和 URL 库，不引入任何子进程生成、网络 I/O 或操作系统特定的执行器代码。
- **严格的反向校验**：提供严格的反序列化防御规则，拒绝未知字段、尾随文档、越界字符与非法状态组合。
- **可复用的安全指纹计算**：提供无副作用的上下文绑定哈希函数（如 `turn_context_fingerprint` 与 `inspection_context_fingerprint`）。
- **内置合规性假宿主 Fixture**：在 `tests/fixtures/host` 中提供参考宿主实现，用于端到端协议合规性验证。

---

## 安装与直接消费

在第三方客户端、网关或 IPC 编解码器中，通过 crates.io 添加轻量依赖：

```toml
[dependencies]
iquji-agent-wire = "0.1.0"
```

在本仓库源码树中开发时，使用工作区路径依赖：

```toml
[dependencies]
iquji-agent-wire = { version = "0.1.0", path = "protocol" }
```

---

## 公开核心类型与接口

| 核心类型 / 函数 | 说明与校验规则 |
| --- | --- |
| `TurnRequest` | 单次回合请求，包含 `agentId`、`prompt` (≤64KB)、可选 `sessionId` (≤512B)、`model`、`attachments` 及最多 32 个 `mcpServers` |
| `TurnOutput` | NDJSON 流条目联合类型，兼具 `TurnEvent`（事件）与 `TurnTerminal`（终态） |
| `TurnEvent` | 流式事件：`SessionStarted { sessionId, resumable }` 或 `TextDelta { text }` |
| `TurnTerminal` | 回合终态：`TurnCompleted { result }` 或 `TurnError { error }` |
| `AgentError` | 稳定的错误代码枚举，涵盖配置、可执行文件、超时、配额、会话与协议等各类失败原因 |
| `ScanReport` | 本地 Agent 扫描报告，包含各 Agent 连接状态与能力描述 |
| `ConfigurationInspection` | 显式配置目录安全检查报告（Provider、模型、凭据环境变量名与指纹） |
| `ProcessExecutionContext` | 原生进程监督上下文（`ownerProcessGroup`），拒绝 0、1 及越界 PID |
| `decode_turn_request` | 将二进制 JSON 严格解析为 `TurnRequest`，拒绝尾随字节与多余字段 |
| `decode_turn_output` | 将单行 NDJSON 解析为 `TurnOutput`，执行完整模式合法性校验 |
| `turn_context_fingerprint` | 计算适配器、可执行体哈希、工作目录与配置目录的 64 位 SHA-256 绑定摘要 |

---

## 使用示例

### 1. 解析与校验 NDJSON 输出流
```rust
use iquji_agent_wire::{TurnOutput, decode_turn_output};

fn process_stream_frame(line: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let output = decode_turn_output(line)?;
    match output {
        TurnOutput::Event(event) => {
            println!("收到事件: {event:?}");
        }
        TurnOutput::Terminal(terminal) => {
            println!("回合结束: {terminal:?}");
        }
    }
    Ok(())
}
```

### 2. 计算不可逆的执行上下文绑定指纹
```rust
use iquji_agent_wire::{Adapter, turn_context_fingerprint};
use std::path::Path;

let fingerprint = turn_context_fingerprint(
    Adapter::CodexCli,
    Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
    Path::new("/canonical/workspace"),
    Some(Path::new("/canonical/codex-config")),
);
println!("执行上下文指纹: {fingerprint}");
```

---

## 协议约束与安全边界

1. **MCP 服务器约束**：
   - 最多声明 32 个名称互不相同的 MCP 服务描述。
   - 每个服务地址必须严格为 `http://127.0.0.1:<port>/mcp/<48位十六进制token>` 回环地址。
   - 不允许包含用户名、密码、Query 参数或 URL Fragment。
2. **工作区写入控制 (`workspaceWrite`)**：
   - 默认且严格为 `false`（只读）。
   - 仅当宿主确认用户授权后才可设置为 `true`。本协议类型不包含任意写入路径，由底层执行器将写入范围收束在已配置的工作目录与临时目录中。
3. **不可逆性与隐私保护**：
   - 上下文指纹、配置检查报告与模型覆盖元数据仅传递文件大小与哈希摘要，不承载绝对文件路径正文、Token 或密码明文。

---

## 测试与合规性验证

在 `protocol` 目录下运行完整单元测试与代码检查：

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

测试集包含独立的假宿主 fixture（位于 `tests/fixtures/host`），在临时隔离目录中离线编译并测试真实标准输入输出通信合同。

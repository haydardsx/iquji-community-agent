# Agent 进程机器合同

`iquji-agent-wire` 提供通用 Agent 请求、事件、终态、扫描与配置元数据的 Rust/Serde 类型、描述验证和假宿主 fixture。它与 Agent 连接实现一起维护，仅依赖精确锁定的 Serde、JSON 编解码器和 SHA-256 实现，不依赖连接实现或产品后端。

## 直接消费

消费者通过 crates.io 使用；本仓源码开发使用路径依赖：

```toml
[dependencies]
iquji-agent-wire = "0.1.0"
```

公开类型包括 `TurnRequest`、`McpServer`、`TurnEvent`、`TurnTerminal`、`TurnOutput`、`TurnResult`、`Attachment`、`ScanReport`、`ConfigurationInspection`、`ModelConfiguration` 和 `AgentError`。`decode_turn_request` 与 `decode_turn_output` 接受单个 JSON 值并执行同一描述验证，拒绝尾随文档、未知字段和未知错误码。扫描报告在 Serde 解码后还须调用 `validate()`。

`ProcessExecutionContext` 是独立的原生进程监督描述，仅包含 `ownerProcessGroup`；其纯验证拒绝 0、1 及超出正 `pid_t` 范围的值。正式命令参数名由 `PROCESS_GROUP_OWNER_ARGUMENT` 提供，实际进程组身份、组长存活与回收由连接实现和原生宿主核验。该描述不属于 `TurnRequest`，不能从模型请求或凭据环境选择。

`TurnRequest.mcpServers` 只接受最多 32 个不同名称的服务描述；为空时省略字段，以兼容早期请求。名称和地址均有字节上限。地址必须是带端口的 `http://127.0.0.1` 回环地址，且路径严格为 `/mcp/<48 位十六进制 token>`；不接受凭据、查询、片段或其他主机。该描述只传递宿主逐回合签发的回环网关地址，不授予访问权；授权、凭据、连接预算与每次工具调用仍由宿主负责。

`TurnRequest.contextFingerprint` 可绑定本回合的执行事实；宿主与工具分别核验后调用纯函数 `turn_context_fingerprint(adapter, executable_sha256, working_directory, configuration_directory)`。CLI 提供规范配置目录，ACP 不提供该目录；执行工作目录始终参与绑定，结果只传 64 位小写 SHA-256。该摘要不代替宿主授权，也不携带路径、凭据或业务身份。

回合 stdout 为 NDJSON，`TurnOutput` 统一读取事件与终态；单回合 Agent 可以省略 `sessionId`，不能伪造可恢复身份。进程监督、读取总期限、合计输出预算、唯一终态与退出状态仍由调用方核验。截断或未完整送达的终态必须失败，不能将局部文本视为成功。

路径描述验证不打开文件，也不签发访问权。执行器须独立核验图片权限、文件身份、摘要、真实能力与读取预算。配置 parser、凭据取值、进程监管和协议 SDK 属于连接实现。

## 型号与配置

型号选择是有界标识，不携带 provider 覆盖、环境值或文件指针。配置检查结果只携带 provider 标识、当前型号、同提供方型号、凭据变量名称与不可逆指纹；`readiness=not_checked` 不表示模型或登录已就绪。

`PreparedModelOverride` 只描述请求型号、patch 及可选 settings 的大小与 SHA-256，每个文件最多 1 MiB，不包含路径或配置正文。它仅伴随 DeepSeek 的成功配置检查；宿主拥有输出目录和文件验证、执行及清理责任。

扫描连接可携带成对的 `contextFingerprint` 与 `executableSha256`，只表达受检制品和执行工作目录绑定。宿主按所选 adapter 和自己已授权的执行工作目录重算回合指纹，CLI 宿主还匹配实际制品与配置根；失败连接不能携带绑定。

## 检查

在本模块目录运行：

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

测试只使用假请求与临时无业务宿主。`tests/fixtures/host` 通过正式 Cargo 依赖和同一机器合同读取 stdin、输出终态；它不读取 CLI 配置、凭据或调用模型。fixture 自持锁文件，测试在临时目录离线编译并真实运行该宿主。

`TurnRequest.workspaceWrite` 默认 false。宿主确认目录授权与执行意图后才设置 true，CLI 实现仅启用配置工作目录的受控写入 sandbox；该字段不提供路径，不改变 provider 或凭据，ACP 实现拒绝写入模式。

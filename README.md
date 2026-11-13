# iquji-agent

通用 Rust Agent 连接工具与库，负责连接和调用本机已授权的外部 Agent。项目包含连接实现和 [protocol 机器合同](protocol/README.md)，不依赖产品后端或其他项目的源码。

本项目按可独立公开的开源项目维护。源码、文档、示例、构建和测试只能依赖本仓内容或任何人均可获取的公开来源；不得依赖仓外工作区路径、未公开源码或凭据。详见[贡献规范](CONTRIBUTING.md)与 [MIT 许可证](LICENSE)。

## 安装

正式版本通过 crates.io 提供：

```sh
cargo add iquji-shared-agent@0.1.0
cargo add iquji-agent-wire@0.1.0
cargo install iquji-shared-agent --version 0.1.0 --locked
```

Rust 依赖只使用公开注册表；源码开发仍使用本仓 protocol 路径。

## 构建

使用 Rust `1.97.1`：

```sh
cargo build --locked
cargo fetch --locked --manifest-path protocol/tests/fixtures/host/Cargo.toml
cargo test --workspace --locked
```

当前受监管进程与文件权限检查支持 Unix；其他平台明确返回不支持。测试只使用临时假 CLI、ACP 进程、配置和图片，不要求安装实际 Agent，不读取用户配置或凭据，不调用认证或真实模型。

## 支持范围

| 接入 | 当前能力 |
| --- | --- |
| Codex CLI | 版本与帮助核验、配置与型号元数据、首轮文本、真实线程续聊、型号选择、PNG/JPEG 图片、逐回合 HTTP MCP 配置 |
| DeepSeek Harness | 版本与帮助核验、headless 配置与型号、单次文本回合、基于正式 `--patch` 的逐回合 HTTP MCP；无会话、续聊、图片或流事件 |
| 原生 ACP / 外部 ACP adapter | 官方 Rust SDK 3.3.0、稳定协议 v1、初始化、文本与图片、真实会话加载、取消，以及仅在声明 HTTP 能力后使用的逐回合 MCP |
| Claude CLI / Grok CLI | 尚未实现，不能根据类型或枚举存在推定可用 |

ACP 每次命令建立新连接；只有对方声明 `loadSession` 才能在后续调用恢复会话。当前不提供 ACP host serve、ACP 型号切换、客户端文件/终端能力或自动认证。Codex 返回已完成的 assistant 文本，不宣称 token 级流式输出。外部 Agent 自身的操作系统沙箱仍由受信本机入口负责。

## 显式配置

将 [config.example.json](config.example.json) 复制为 `.local/config.json`，替换为已授权的绝对可执行路径和规范工作目录：

```sh
mkdir -p .local
chmod 700 .local
cp config.example.json .local/config.json
chmod 600 .local/config.json
```

编辑配置后运行：

```sh
cargo run --locked -- --config "$PWD/.local/config.json" scan
```

配置文件须由当前用户持有，Unix 权限为 `0600`，父目录由当前用户持有且不可由组或其他用户写入。CLI 不搜索当前目录、PATH 或环境变量来猜测配置或入口。显式入口可以是符号链接安装方式，最终目标仍须为普通可执行文件；可选 `expectedSha256` 按实际目标核对。

顶层接受 `schemaVersion`、`workingDirectory`、`agents` 和可选的 `timeoutMs`、`maxOutputBytes`、`turnTimeoutMs`、`maxTurnOutputBytes`。每个 Agent 包含 `id`、`adapter`、`executable`，可选 `expectedSha256`；CLI 还接受匹配 adapter 的 `codexConfigurationDirectory` 或 `deepseekConfigurationDirectory`，以及 `environmentVariables` 名称列表。不接受外部 argv、Shell 字符串、环境值或临时命令。

配置目录可显式选择现有 provider、型号与中转设置；工具不改写用户原件，不执行登录检查，不将第三方 provider 的可用性绑定到官方登录。需要凭据或代理时，`environmentVariables` 只登记原生进程中的变量名称，值不写入配置、报告或诊断。工具控制的 HOME、CLI 配置目录变量与危险的注入环境变量不可覆盖。本机配置、环境和运行目录保持 Git 忽略。

## 扫描与配置检查

```sh
cargo run --locked -- --config "$PWD/.local/config.json" scan local-codex
cargo run --locked -- --config "$PWD/.local/config.json" inspect local-codex
```

`scan` 只执行固定版本与帮助参数或 ACP `initialize`；不创建会话、不认证、不调用模型。省略 Agent 身份时扫描全部配置项。元数据命令使用工具自身创建的空 HOME/CLI 配置目录，结束后清理，不读取用户凭据。

`inspect` 只检查显式声明的 CLI 配置目录；省略身份时检查全部配置项。结果可包含选中 provider、当前及同 provider 型号、凭据环境变量名称和指纹，不返回配置正文、目录、provider URL 或秘密值。Codex 不读取 `auth.json`；DeepSeek 使用固定 `--profile headless --dump-config` 读取有效配置。DeepSeek settings provider 未指定 `path` 或 `dshHome` 时，缺失的默认 `settings.yaml` 按官方语义作为空设置，保留组合层选择且不读取迁移备份；显式引用缺失、不安全或畸形内容仍拒绝。没有配置目录时返回 `configuration_metadata_unavailable`，不回退 HOME。

两种报告的 `readiness` 始终为 `not_checked`。扫描的 `detected` 只表示真实版本、参数或协议已核验，不保证凭据、网络、模型或业务操作可用。未实现和能力不足会返回稳定错误，不伪造成功。

## 单次回合

`turn` 从 stdin 接受唯一的 `TurnRequest` JSON，以 EOF 结束：

```sh
printf '%s\n' '{"agentId":"local-codex","prompt":"本次问题"}' | \
  cargo run --locked -- --config "$PWD/.local/config.json" turn
```

请求包含 `agentId`、文本 `prompt`，可选 `sessionId`、`model`、`contextFingerprint`、`workspaceWrite`、`attachments` 和最多 32 项的 `mcpServers`。MCP 项只接受由宿主签发的高熵 IPv4 回环 HTTP 网关地址；宿主继续负责授权、凭据和传输。请求不能传入命令、环境值或工作目录。文本最多 64 KiB，输入 JSON 最多 400 KiB，不透明会话身份最多 512 字节；Codex 线程身份另外限制为最多 200 字节的安全 ASCII 标识。

stdout 为 NDJSON，按 adapter 能力输出 `session_started`、`text_delta`，并以唯一 `turn_completed` 或 `turn_error` 结束。成功退出码为 `0`；错误退出码为 `2`，诊断只使用稳定错误码。DeepSeek 仅输出最终成功对象，没有会话或流事件。消费者须同时检查完整终态、退出状态与 EOF；部分文本不能表示成功。

回合默认只读。宿主完成目录授权后可以显式设置 `workspaceWrite: true`，Codex 使用 `workspace-write` sandbox，DeepSeek 使用 headless 的 `workspace-write` 权限预设，源码写入限制在配置工作目录及 CLI 的临时目录。工作目录始终由可信配置提供。ACP 权限请求统一取消，不向 Agent 提供客户端文件或终端能力。回合不会自动登录、修改 provider 或执行宿主业务授权。

DeepSeek `0.2.0` 起，可信配置已明确授权现存 DSH 目录时，每个回合按官方 headless 合同在私有 `--patch` 中恢复已有存储认证；仅启用身份匹配的 `credentials`、`authorization` 和 `deepseek-account` 三个正式条目。当前按 `0.2.2` 的正式组件核验：分别为 `@deepseek-ai/dsh-credentials-local`、`@deepseek-ai/dsh-authorization` 和 `@deepseek-ai/dsh-deepseek-account`，拒绝重复 ID 或替换组件。官方 API key 模型设置读取 `@deepseek-ai/dsh-llm-deepseek-api-key`。默认就绪检查同样适用，不改变 provider、默认型号、用户 profile 或只读权限。结构化的 `version`、`refs` 和 `records` 凭据由官方 CLI 自行消费，工具不读取其正文。

DeepSeek `0.1.x` 的官方 `credentials-local` 在启动时拒绝其不支持的非字符串值，且错误包含正式解析器与加载器堆栈时，仅投影 `configuration_metadata_invalid`；这不表示该凭据文档对新版 CLI 无效。普通 `TypeError` 或缺失堆栈仍为 `agent_request_failed`。原始路径、凭据名称与错误正文不返回调用方，工具不读取或修复凭据文件。

Codex 在线程建立后、回合开始前输出的非终态 `item.completed` 错误诊断只在内部忽略，不进入响应；成功仍要求真实回合开始、完整 assistant 文本与回合完成。顶层 `error`、`turn.failed` 和提前出现的 assistant 消息仍失败。

回合默认限时 60 秒、stdout/stderr 合计 1 MiB。本机配置允许 10 毫秒至 10 分钟、128 字节至 4 MiB。检查、stdin、执行、输出和清理共用有界预算；取消、超时、超限或失败先回收受监管进程组与入口进程。工具只清理自己创建并核验身份的临时目录，不删除用户配置目录。

库消费者可调用 `run_turn(configuration, request, cancelled, on_event)`，并复用公开导出的请求、附件、扫描、事件、终态和错误类型。回调应及时返回，由宿主管理输出背压；具体数据定义见 `protocol`。

## 外部进程组监督

独立使用时，工具继续为每个 CLI 创建并回收自己的进程组。原生宿主已经建立根进程组并负责整组清理时，可在 `scan`、`inspect` 或 `turn` 命令末尾显式追加 `--process-group-owner <组长 PID>`；库消费者通过 `Configuration::with_process_context(ProcessExecutionContext)` 装配同一内存上下文。工具逐次核验自身属于该组、自己不是组长且外部组长仍存活，CLI 继承该组，不创建嵌套进程组。

该上下文不能来自配置 JSON、环境变量或 `TurnRequest`，不授予业务权限。继承模式中工具仅回收自己持有的直接入口，不向整个外部组发送信号；宿主必须在取消、超时、异常及一次执行完成后回收整个根组，不能把不确定的入口重新当作健康会话。EOF、输出预算与墙钟期限仍按相同合同核验，不因进程继承放宽。

## 型号与图片

可选 `model` 仅覆盖本回合型号，不覆盖 provider 或环境。Codex 通过真实帮助声明的独立参数选择型号；DeepSeek 在自有私有目录创建临时覆盖，保留当前 provider、中转和其余设置，终止后清理。

需要自行核验 DeepSeek 型号覆盖的宿主可使用：

```sh
cargo run --locked -- --config "$PWD/.local/config.json" inspect local-deepseek \
  --model model-id --output-directory /absolute/private-output
```

该入口只适用于已选中的 DeepSeek。输出目录须为调用者创建的空、规范、当前用户持有的 `0700` 目录。报告仅提供覆盖文件大小与 SHA-256；调用方负责独立核验、使用和清理自己持有的文件。

图片只接受 `kind=image` 的 PNG/JPEG，最多 3 项、单项最多 4 MiB、合计最多 12 MiB。附件包含规范绝对 `path`、`mediaType`、`size` 和无前缀的 64 位小写十六进制 `sha256`。输入应为调用方准备的私有快照：当前用户持有的普通 `0400` 文件，直接父目录为当前用户持有的 `0700` 目录。

工具核对文件身份、权限、格式签名、大小与完整摘要，拒绝符号链接或变化的输入。Codex 使用工具自身冻结的图片暂存，ACP 通过 SDK 发送已核验的 Base64 与 MIME；两者都须先声明真实图片能力。DeepSeek 在启动前拒绝附件。图片格式检查不解压像素，也不保证某个模型一定接受图片。

## 发布

两个 crate 统一使用 `0.1.0` 等版本，`v` 前缀标签必须与版本一致。维护者首次登录 crates.io 并验证邮箱后发布；后续为两个 crate 配置可信发布，仓库为 `haydardsx/iquji-community-agent`、工作流为 `release.yml`、环境为 `crates-io`。工作流先完成 workspace 格式、测试、Clippy 和打包，再按 wire、agent 顺序发布；已存在版本不覆盖，重跑继续未发布的 crate。

`cargo package --workspace --locked` 同时校验两个待发布 crate，制品仅包含正式源码、说明、配置示例与许可，不包含测试夹具、工作区本机配置或构建产物。测试夹具保持 `publish = false`。当前不单独发行预编译二进制。

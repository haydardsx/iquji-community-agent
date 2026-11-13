# iquji-agent

[简体中文](README.md) | English

A general-purpose Rust tool and library for connecting to and invoking external Agents already authorized on the local machine. This project includes the connection implementation and the [protocol contract](protocol/README.md). It does not depend on a product backend or source code from other projects.

This project is maintained as independently usable open source. Source code, documentation, examples, builds, and tests may depend only on this repository or publicly available sources. They must not depend on external workspace paths, unpublished source code, or credentials. See the [contribution guide](CONTRIBUTING.md) and [MIT License](LICENSE).

## Installation

Release versions are available from crates.io:

```sh
cargo add iquji-shared-agent@0.1.0
cargo add iquji-agent-wire@0.1.0
cargo install iquji-shared-agent --version 0.1.0 --locked
```

Rust dependencies use only public registries. Source development continues to use the `protocol` path within this repository.

## Building

Use Rust `1.97.1`:

```sh
cargo build --locked
cargo fetch --locked --manifest-path protocol/tests/fixtures/host/Cargo.toml
cargo test --workspace --locked
```

Supervised process execution and file-permission checks currently support Unix. Other platforms explicitly return an unsupported error. Tests use only temporary fake CLIs, ACP processes, configurations, and images. They do not require real Agents to be installed, read user configuration or credentials, authenticate, or call real models.

## Supported integrations

| Integration | Current capabilities |
| --- | --- |
| Codex CLI | Version and help verification, configuration and model metadata, initial text turns, continuation of real threads, model selection, PNG/JPEG images, and per-turn HTTP MCP configuration |
| DeepSeek Harness | Version and help verification, headless configuration and models, single text turns, and per-turn HTTP MCP using the official `--patch` interface; no sessions, continuation, images, or streaming events |
| Native ACP / external ACP adapter | Official Rust SDK 3.3.0, stable protocol v1, initialization, text and images, loading real sessions, cancellation, and per-turn MCP only after HTTP support has been declared |
| Claude CLI / Grok CLI | Not implemented; the existence of a type or enum does not imply availability |

Each ACP command opens a new connection. A later invocation can restore a session only if the peer declares `loadSession`. ACP host serving, ACP model switching, client file/terminal capabilities, and automatic authentication are not currently provided. Codex returns completed assistant text and does not claim token-level streaming. The trusted local entry point remains responsible for the external Agent's own operating-system sandbox.

## Explicit configuration

Copy [config.example.json](config.example.json) to `.local/config.json`, then replace the placeholders with authorized absolute executable paths and a canonical working directory:

```sh
mkdir -p .local
chmod 700 .local
cp config.example.json .local/config.json
chmod 600 .local/config.json
```

After editing the configuration, run:

```sh
cargo run --locked -- --config "$PWD/.local/config.json" scan
```

The configuration file must be owned by the current user and have Unix permissions `0600`. Its parent directory must be owned by the current user and must not be writable by the group or others. The CLI does not search the current directory, `PATH`, or environment variables to infer configuration or an entry point. An explicitly configured entry point may be installed through a symbolic link, but its final target must be a regular executable file. If supplied, `expectedSha256` is checked against the actual target.

The top-level configuration accepts `schemaVersion`, `workingDirectory`, and `agents`, plus optional `timeoutMs`, `maxOutputBytes`, `turnTimeoutMs`, and `maxTurnOutputBytes`. Each Agent contains `id`, `adapter`, and `executable`, with optional `expectedSha256`. CLI integrations also accept an adapter-matching `codexConfigurationDirectory` or `deepseekConfigurationDirectory`, and a list of `environmentVariables` names. External argv, shell strings, environment values, and ad hoc commands are not accepted.

A configuration directory can explicitly select existing provider, model, and relay settings. The tool does not rewrite the user's originals, perform login checks, or make third-party provider availability depend on an official login. When credentials or a proxy are required, `environmentVariables` records only variable names from the native process environment; values are not written to configuration, reports, or diagnostics. The tool-controlled `HOME`, CLI configuration-directory variables, and dangerous environment-injection variables cannot be overridden. Local configuration, environment files, and runtime directories remain Git-ignored.

## Scanning and configuration inspection

```sh
cargo run --locked -- --config "$PWD/.local/config.json" scan local-codex
cargo run --locked -- --config "$PWD/.local/config.json" inspect local-codex
```

`scan` executes only fixed version/help arguments or ACP `initialize`. It does not create sessions, authenticate, or invoke models. Omitting the Agent ID scans all configured entries. Metadata commands use empty `HOME` and CLI configuration directories created by the tool and removed afterward; they do not read user credentials.

`inspect` examines only explicitly declared CLI configuration directories. Omitting the Agent ID inspects all configured entries. Results may include the selected provider, the current model and other models from the same provider, credential environment-variable names, and fingerprints. They do not return configuration contents, directories, provider URLs, or secret values. Codex does not read `auth.json`; DeepSeek reads effective configuration using the fixed `--profile headless --dump-config` arguments. When a DeepSeek settings provider specifies neither `path` nor `dshHome`, a missing default `settings.yaml` is treated as empty settings according to the official semantics. The composition-layer selection is preserved, and migration backups are not read. Missing explicitly referenced files, unsafe inputs, and malformed content are still rejected. Without a configuration directory, the result is `configuration_metadata_unavailable`; there is no fallback to `HOME`.

Both reports always set `readiness` to `not_checked`. A scan result of `detected` means only that the actual version, arguments, or protocol have been verified. It does not guarantee that credentials, networking, models, or business operations are usable. Unimplemented integrations and insufficient capabilities return stable errors rather than fabricated success.

## A single turn

`turn` accepts exactly one `TurnRequest` JSON object from stdin, terminated by EOF:

```sh
printf '%s\n' '{"agentId":"local-codex","prompt":"Question for this turn"}' | \
  cargo run --locked -- --config "$PWD/.local/config.json" turn
```

The request includes `agentId` and a text `prompt`, with optional `sessionId`, `model`, `contextFingerprint`, `workspaceWrite`, `attachments`, and up to 32 `mcpServers` entries. MCP entries accept only host-issued, high-entropy IPv4 loopback HTTP gateway addresses. The host remains responsible for authorization, credentials, and transport. Requests cannot supply commands, environment values, or working directories. Text is limited to 64 KiB, input JSON to 400 KiB, and opaque session IDs to 512 bytes. Codex thread IDs are additionally limited to safe ASCII identifiers of at most 200 bytes.

stdout is NDJSON. Depending on adapter capabilities, it emits `session_started` and `text_delta`, then ends with exactly one `turn_completed` or `turn_error`. The success exit code is `0`; the error exit code is `2`. Diagnostics use only stable error codes. DeepSeek emits only the final success object, with no session or streaming events. Consumers must check the complete terminal result, exit status, and EOF together. Partial text does not indicate success.

Turns are read-only by default. After authorizing the directory, the host may explicitly set `workspaceWrite: true`. Codex then uses the `workspace-write` sandbox, and DeepSeek uses the headless `workspace-write` permission preset. Source writes are limited to the configured working directory and the CLI's temporary directories. The working directory always comes from trusted configuration. ACP permission requests are uniformly cancelled, and no client file or terminal capabilities are offered to the Agent. A turn does not automatically log in, change providers, or perform the host's business authorization.

Starting with DeepSeek `0.2.0`, when trusted configuration explicitly authorizes an existing DSH directory, each turn restores existing stored authentication through a private `--patch` according to the official headless contract. Only the three official entries `credentials`, `authorization`, and `deepseek-account` with matching identities are enabled. Verification currently follows the official `0.2.2` components: `@deepseek-ai/dsh-credentials-local`, `@deepseek-ai/dsh-authorization`, and `@deepseek-ai/dsh-deepseek-account`, respectively. Duplicate IDs and substituted components are rejected. Official API-key model settings are read from `@deepseek-ai/dsh-llm-deepseek-api-key`. The default readiness checks still apply; the provider, default model, user profile, and read-only permissions are not changed. The official CLI consumes structured `version`, `refs`, and `records` credentials itself; this tool does not read their contents.

When DeepSeek `0.1.x`'s official `credentials-local` rejects unsupported non-string values at startup, the error is mapped to `configuration_metadata_invalid` only if it includes the official parser and loader stack. This does not mean the credential document is invalid for newer CLI versions. An ordinary `TypeError` or a missing stack remains `agent_request_failed`. Raw paths, credential names, and error contents are not returned to callers. The tool neither reads nor repairs credential files.

Codex may emit non-terminal `item.completed` error diagnostics after creating a thread and before starting a turn. These diagnostics are ignored internally and are not included in the response. Success still requires a real turn start, complete assistant text, and turn completion. A top-level `error`, `turn.failed`, or an assistant message arriving before the turn starts still causes failure.

The default turn timeout is 60 seconds, and combined stdout/stderr is limited to 1 MiB. Local configuration allows 10 milliseconds to 10 minutes and 128 bytes to 4 MiB. Validation, stdin, execution, output, and cleanup share a bounded budget. Cancellation, timeout, limit violations, or failure first trigger cleanup of the supervised process group and entry process. The tool removes only temporary directories it created and whose identity it has verified; it does not delete user configuration directories.

Library consumers can call `run_turn(configuration, request, cancelled, on_event)` and reuse the publicly exported request, attachment, scan, event, terminal-result, and error types. Callbacks should return promptly; the host manages output backpressure. See `protocol` for the data definitions.

## External process-group supervision

When used independently, the tool creates and cleans up its own process group for each CLI. If a native host has already created a root process group and owns cleanup of the entire group, it can explicitly append `--process-group-owner <group-leader-PID>` to a `scan`, `inspect`, or `turn` command. Library consumers supply the same in-memory context through `Configuration::with_process_context(ProcessExecutionContext)`. On each invocation, the tool verifies that it belongs to the specified group, is not the group leader, and that the external group leader is still alive. The CLI inherits that group; no nested process group is created.

This context cannot come from configuration JSON, environment variables, or `TurnRequest`, and it grants no business permissions. In inherited-group mode, the tool cleans up only the direct entry process it owns and does not signal the whole external group. The host must clean up the entire root group on cancellation, timeout, abnormal termination, and after each execution completes. An entry process with uncertain state must not be reused as a healthy session. EOF, output budgets, and wall-clock deadlines are checked under the same contract and are not relaxed by process-group inheritance.

## Models and images

The optional `model` overrides only the model for the current turn, not the provider or environment. Codex selects a model using a separate argument declared by its actual help output. DeepSeek creates a temporary override in a private directory owned by the tool, preserving the current provider, relay, and other settings, then removes it on termination.

Hosts that need to independently verify a DeepSeek model override can run:

```sh
cargo run --locked -- --config "$PWD/.local/config.json" inspect local-deepseek \
  --model model-id --output-directory /absolute/private-output
```

This entry point applies only to the selected DeepSeek integration. The output directory must be an empty, canonical directory created and owned by the caller with permissions `0700`. The report provides only the override file's size and SHA-256. The caller is responsible for independently verifying, using, and cleaning up the files it owns.

Images must use `kind=image` and be PNG or JPEG. At most three images are accepted, with a maximum of 4 MiB each and 12 MiB in total. Attachments contain a canonical absolute `path`, `mediaType`, `size`, and `sha256` as 64 lowercase hexadecimal characters without a prefix. Inputs should be private snapshots prepared by the caller: regular `0400` files owned by the current user, with an immediate parent directory owned by the current user and permissions `0700`.

The tool checks file identity, permissions, format signature, size, and the complete digest, rejecting symbolic links and inputs that change. Codex uses image staging frozen by the tool; ACP sends verified Base64 and MIME data through the SDK. Both integrations must first declare actual image support. DeepSeek rejects attachments before startup. Image-format checks do not decompress pixels and do not guarantee that any particular model will accept the image.

## Publishing

Updates to `main` trigger `auto-release.yml`; maintainers can also dispatch that workflow to retry. The planner processes unfinished automatic tags serially, then reads the latest `main` without replaying older main commits. Each source commit receives at most one version. The next version increments the patch of the highest stable version found in the source baseline, existing stable tags, and either crate's registry history. Source files retain their development baseline. Version changes exist only in a separate snapshot commit whose parent is the source commit, updating both crates, their internal dependency, and both lockfiles without writing back to `main`.

Only after formatting, tests, Clippy, and package validation succeed does the planner create an immutable `v` tag. Its `Auto-Release-Source` annotation identifies the source commit. The planner explicitly dispatches `release.yml` and waits for it because tags pushed with GitHub's automatic token do not trigger another workflow. Configure trusted publishing for both crates with repository `haydardsx/iquji-community-agent`, workflow `release.yml`, and environment `crates-io`; that environment allows only `v*` tags.

The publisher completes checks on both supported CI operating systems and warms the build and package outputs before requesting a short-lived OIDC credential. It publishes wire before agent. An existing version is skipped only when its checksum matches exactly. A GitHub Release marks completion only after both crates are readable on crates.io with matching checksums. Failures leave the tag available for the next planner run to resume; tags and published artifacts are never overwritten. Retrying an older version cannot move the GitHub Release latest marker backwards.

`cargo package --workspace --locked` validates both crates to be published. Artifacts contain only release source code, documentation, configuration examples, and licenses. They exclude test fixtures, local workspace configuration, and build outputs. Test fixtures retain `publish = false`. Prebuilt binaries are not currently released separately.

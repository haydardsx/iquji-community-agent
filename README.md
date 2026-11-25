# IQuji Community Agent

[简体中文](README.zh-CN.md)

IQuji Community Agent is a Rust library and CLI for developers connecting applications or scripts to coding agents already configured on their machine. Check which agents are available, inspect their capabilities, and send prompts through the Agent Client Protocol (ACP) or adapters for Codex CLI and DeepSeek Harness. Adapters that support streaming deliver response updates as they arrive.

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

## Contributing

Start from the latest `origin/main` on a purpose-named branch such as `feat/agent-discovery`. Use a Conventional Commits type followed by `/` and a topic of lowercase ASCII letters and digits separated by single hyphens. The full name is at most 64 characters.

```sh
python3 scripts/check_staged.py --install
```

Stage the intended changes, then commit. The installed hook runs the repository's CI commands in a temporary checkout of the staged snapshot. You can run `python3 scripts/check_staged.py` directly to check the snapshot before committing. If you already have hooks, call this command from `pre-commit` and call `python3 .github/scripts/check_subjects.py --commit-message "$1"` from `commit-msg`.

Use `type(scope): describe the change` for each commit and the pull request title. Supported scopes are `agent`, `cli`, `wire`, `protocol`, `config`, `runtime`, `docs`, `deps`, `release`, `ci`, `test`, and `build`. Reviewed commits use the project maintenance identity `IQuji Community <community@iqu.lol>` for both the author and the committer. Open pull requests against `main` and wait for the required checks.

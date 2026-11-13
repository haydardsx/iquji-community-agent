# 贡献规范

本仓的所有跟踪文件均按公开内容维护。提交源码、文档、配置、构建和测试资源时，确保它们适合公开发布，并能从本仓的干净检出中读取和验证。

工程依赖只能来自本仓或任何人均可获取的公开来源。不得依赖检出目录之外的本地路径、未公开源码、私有包、凭据或需要组织授权的服务。文档和示例描述本项目自身，使用从仓库根目录可执行的命令，不记录父级工作区的挂载位置或内部结构。

提交前运行：

```sh
cargo fmt -- --check
cargo fmt --manifest-path protocol/Cargo.toml -- --check
cargo test --locked
cargo test --locked --manifest-path protocol/Cargo.toml
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --manifest-path protocol/Cargo.toml --all-targets -- -D warnings
```

本仓采用 MIT License。贡献内容须由提交者创作，或已获得在 MIT License 下发布所需的授权；第三方内容须保留其许可和归属说明。

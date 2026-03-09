## Installing & building

### System requirements

| Requirement                 | Details                                                                                                                                                   |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Operating systems           | macOS 12+, Linux (the shell installer prefers `legacy-musl`/`musl` assets when available; `gnu` assets require glibc >= 2.35), Windows 11 **via WSL2** for the core workspace, or Windows PowerShell for `hodexctl` release management |
| Git (optional, recommended) | 2.23+ for built-in PR helpers                                                                                                                             |
| RAM                         | 4-GB minimum (8-GB recommended)                                                                                                                           |

> **Note:** The shell installer prefers `legacy-musl`, then `musl`, then `gnu` Linux assets when available. `gnu` assets require glibc 2.35 or newer.

### Hodexctl

如果你想独立管理 `hodex` 的 release 版本，同时不影响现有 `codex`，可以使用 `hodexctl`。

推荐首装方式：

```bash
curl -fsSL https://raw.githubusercontent.com/stellarlinkco/codex/main/scripts/install-hodexctl.sh | bash
```

```powershell
irm https://raw.githubusercontent.com/stellarlinkco/codex/main/scripts/install-hodexctl.ps1 | iex
```

安装完成后，后续统一使用：

```bash
hodexctl
hodexctl install
hodexctl list
```

常见用法：

```bash
./scripts/hodexctl/hodexctl.sh install
hodexctl list
hodexctl upgrade
hodexctl status
```

```powershell
.\scripts\hodexctl\hodexctl.ps1 install
hodexctl list
hodexctl upgrade
hodexctl status
```

说明：

- `hodex` 只管理 release。
- `hodexctl source ...` 只负责源码下载、同步和工具链准备。
- `hodexctl uninstall` 不会影响原有 `codex`。
- 详细参数和交互说明见 [../scripts/hodexctl/README.md](../scripts/hodexctl/README.md)。

### DotSlash

The GitHub Release also contains a [DotSlash](https://dotslash-cli.com/) file for the Codex CLI named `codex`. Using a DotSlash file makes it possible to make a lightweight commit to source control to ensure all contributors use the same version of an executable, regardless of what platform they use for development.

### Build from source

```bash
# Clone the repository and navigate to the root of the Cargo workspace.
git clone https://github.com/stellarlinkco/codex.git
cd codex/codex-rs

# Install the Rust toolchain, if necessary.
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustup component add rustfmt
rustup component add clippy
# Install helper tools used by the workspace justfile:
cargo install just
# Optional: install nextest for the `just test` helper
cargo install --locked cargo-nextest

# Build Codex.
cargo build

# Launch the TUI with a sample prompt.
cargo run --bin codex -- "explain this codebase to me"

# After making changes, use the root justfile helpers (they default to codex-rs):
just fmt
just fix -p <crate-you-touched>

# Run the relevant tests (project-specific is fastest), for example:
cargo test -p codex-tui
# If you have cargo-nextest installed, `just test` runs the test suite via nextest:
just test
# Avoid `--all-features` for routine local runs because it increases build
# time and `target/` disk usage by compiling additional feature combinations.
# If you specifically want full feature coverage, use:
cargo test --all-features
```

## Tracing / verbose logging

Codex is written in Rust, so it honors the `RUST_LOG` environment variable to configure its logging behavior.

The TUI defaults to `RUST_LOG=codex_core=info,codex_tui=info,codex_rmcp_client=info` and log messages are written to `~/.codex/log/codex-tui.log` by default. For a single run, you can override the log directory with `-c log_dir=...` (for example, `-c log_dir=./.codex-log`).

```bash
tail -F ~/.codex/log/codex-tui.log
```

By comparison, the non-interactive mode (`codex exec`) defaults to `RUST_LOG=error`, but messages are printed inline, so there is no need to monitor a separate file.

See the Rust documentation on [`RUST_LOG`](https://docs.rs/env_logger/latest/env_logger/#enabling-logging) for more information on the configuration options.

<p align="center"><strong>Codex (fork)</strong> is a Rust-first coding agent forked from <a href="https://github.com/openai/codex">openai/codex</a>.</p>
<p align="center">This fork aims to match Claude Code-style workflows: <strong>agent teams</strong>, <strong>hooks</strong>, <strong>Anthropic API agent</strong>, and a <strong>Web UI</strong> served by <code>codex serve</code>.</p>
<p align="center">Goal: a Rust <strong>OpenCode</strong> with multi-model support, multi-agent collaboration, and long-running orchestration.</p>
<p align="center">
  <img src="https://github.com/openai/codex/blob/main/.github/codex-cli-splash.png" alt="Codex CLI splash" width="80%" />
</p>

## Sponsor

<table>
<tr>
<td width="180"><a href="https://www.packyapi.com/register?aff=wZPe"><img src="assets/partners/logos/packycode.png" alt="PackyCode" width="150"></a></td>
<td>Thanks to PackyCode for sponsoring this project! PackyCode is a reliable and efficient API relay service provider, offering relay services for Claude Code, Codex, Gemini, and more. PackyCode provides special discounts for our software users: register using <a href="https://www.packyapi.com/register?aff=wZPe">this link</a> and enter the "houcode" promo code during first recharge to get 10% off.</td>
</tr>
</table>

---

## Quickstart

### Install (latest GitHub Release)

**macOS, Linux, WSL:**

```shell
curl -fsSL https://raw.githubusercontent.com/stellarlinkco/codex/main/scripts/install.sh | bash
```

The shell install command above prefers `gnu` release assets on Linux and falls back to `musl` when needed; `gnu` builds require glibc >= 2.35 (Ubuntu 22.04+).

**Windows PowerShell:**

```powershell
irm https://raw.githubusercontent.com/stellarlinkco/codex/main/scripts/install.ps1 | iex
```

Copy/paste the command for your platform above to download the latest Release binary for your OS/arch. The shell command installs `codex` to `~/.local/bin` and prints a PATH reminder.

### Optional: Hodexctl

Use `hodexctl` if you want to manage `hodex` separately without affecting an existing `codex` install.

**macOS, Linux, WSL:**

```shell
curl -fsSL https://raw.githubusercontent.com/stellarlinkco/codex/main/scripts/install-hodexctl.sh | bash
```

**Windows PowerShell:**

```powershell
irm https://raw.githubusercontent.com/stellarlinkco/codex/main/scripts/install-hodexctl.ps1 | iex
```

After install, use `hodexctl`, `hodexctl install`, or `hodexctl list`.
More details: [Hodexctl guide](./scripts/hodexctl/README.md).

### Run

```shell
codex --version
codex serve
```

### Telegram on macOS

Codex now supports a macOS-only Telegram control surface for `codex serve`.

Quick start:

```shell
export CODEX_TELEGRAM_BOT_TOKEN="123456:telegram-token"
export CODEX_TELEGRAM_ALLOWED_CHAT_IDS="123456789"
codex serve --host 127.0.0.1 --port 8787
```

What it adds:

- view projects and sessions from Telegram
- attach to a running local CLI or TUI session
- open dormant or archived sessions in read-only mode, then continue them on demand
- stream assistant output into Telegram while the local terminal stays in sync
- approve requests and answer `request_user_input` directly from Telegram

Telegram commands:

- `/projects`
- `/refresh`
- `/continue`
- `/stop`

See [**Telegram Bot for Codex on macOS**](./docs/telegram-bot-macos.md) for the full setup, troubleshooting, and end-to-end validation flow.

## This Update

Recent work in this fork includes:

- macOS live bridge and runtime registry for running CLI or TUI sessions
- single-owner lease handling and stale lease recovery
- unified `liveWindow` / `headless` / `stored` session aggregation in `codex serve`
- live owner delta streaming and remote control routing
- Telegram bot worker integration with watched-session UX and throttled streaming updates
- operations documentation for Telegram setup and validation

## Docs

- [**Contributing**](./docs/contributing.md)
- [**Installing & building**](./docs/install.md)
- [**Telegram Bot for Codex on macOS**](./docs/telegram-bot-macos.md)
- [**Hodexctl guide**](./scripts/hodexctl/README.md)
- [**Open source fund**](./docs/open-source-fund.md)

## Acknowledgements

- https://github.com/openai/codex
- https://github.com/tiann/hapi

This repository is licensed under the [AGPL-3.0 License](LICENSE).

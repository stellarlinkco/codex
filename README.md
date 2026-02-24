<p align="center"><code>curl -fsSL https://raw.githubusercontent.com/stellarlinkco/codex/main/scripts/install.sh | bash</code><br />or <code>irm https://raw.githubusercontent.com/stellarlinkco/codex/main/scripts/install.ps1 | iex</code></p>
<p align="center">一键复制安装（自动安装最新 Release 版本）</p>
<p align="center">Sponsor: <strong>PackyCode</strong></p>
<p align="center"><strong>Codex (fork)</strong> is a Rust-first coding agent forked from <a href="https://github.com/openai/codex">openai/codex</a>.</p>
<p align="center">This fork aims to match Claude Code-style workflows: <strong>agent teams</strong>, <strong>hooks</strong>, <strong>Anthropic API agent</strong>, and a <strong>Web UI</strong> served by <code>codex serve</code>.</p>
<p align="center">Goal: a Rust <strong>OpenCode</strong> with multi-model support, multi-agent collaboration, and long-running orchestration.</p>
<p align="center">
  <img src="https://github.com/openai/codex/blob/main/.github/codex-cli-splash.png" alt="Codex CLI splash" width="80%" />
</p>

---

## Quickstart

### Install (latest GitHub Release)

```shell
curl -fsSL https://raw.githubusercontent.com/stellarlinkco/codex/main/scripts/install.sh | bash
```

```powershell
irm https://raw.githubusercontent.com/stellarlinkco/codex/main/scripts/install.ps1 | iex
```

一键复制上面任意命令，即可自动下载与你系统匹配的最新 Release 二进制并安装到 `~/.local/bin`。

### Run

```shell
codex --version
codex serve
```

## Docs

- [**Contributing**](./docs/contributing.md)
- [**Installing & building**](./docs/install.md)
- [**Open source fund**](./docs/open-source-fund.md)

## Acknowledgements

- https://github.com/openai/codex
- https://github.com/tiann/hapi

This repository is licensed under the [Apache-2.0 License](LICENSE).

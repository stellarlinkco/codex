<p align="center"><strong>Codex (fork)</strong> is a Rust-first coding agent forked from <a href="https://github.com/openai/codex">openai/codex</a>.</p>
<p align="center">This fork aims to match Claude Code-style workflows: <strong>agent teams</strong>, <strong>hooks</strong>, <strong>Anthropic API agent</strong>, and a <strong>Web UI</strong> served by <code>codex serve</code>.</p>
<p align="center">Goal: a Rust <strong>OpenCode</strong> with multi-model support, multi-agent collaboration, and long-running orchestration.</p>
<p align="center">
  <img src="https://github.com/openai/codex/blob/main/.github/codex-cli-splash.png" alt="Codex CLI splash" width="80%" />
</p>

## ❤️ Sponsor

<table>
<tr>
<td width="180"><a href="https://www.packyapi.com/"><strong>PackyCode</strong></a></td>
<td>Thanks to PackyCode for sponsoring this project! PackyCode is a reliable and efficient API relay service provider, offering relay services for Claude Code, Codex, Gemini, and more.</td>
</tr>
</table>

---

## Quickstart

### Install (latest GitHub Release)

```shell
curl -fsSL https://raw.githubusercontent.com/stellarlinkco/codex/main/scripts/install.sh | bash
```

```powershell
irm https://raw.githubusercontent.com/stellarlinkco/codex/main/scripts/install.ps1 | iex
```

Copy/paste either command above to download the latest Release binary for your OS/arch and install it to `~/.local/bin`.

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

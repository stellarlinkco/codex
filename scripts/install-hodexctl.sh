#!/usr/bin/env bash
set -euo pipefail

repo="${HODEXCTL_REPO:-${CODEX_REPO:-stellarlinkco/codex}}"
controller_url_base="${HODEX_CONTROLLER_URL_BASE:-https://raw.githubusercontent.com}"
state_dir="${HODEX_STATE_DIR:-$HOME/.hodex}"
command_dir="${HODEX_COMMAND_DIR:-${INSTALL_DIR:-}}"
controller_url="${controller_url_base%/}/${repo}/main/scripts/hodexctl/hodexctl.sh"

if ! command -v curl >/dev/null 2>&1; then
  echo "Missing dependency: curl" >&2
  exit 1
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
controller_path="$tmp_dir/hodexctl.sh"

printf '==> 下载 hodexctl 管理脚本\n'
curl -fsSL "$controller_url" -o "$controller_path"
chmod +x "$controller_path"
printf '==> 启动 hodexctl 首次安装\n'

args=(manager-install --yes --state-dir "$state_dir" --repo "$repo")

if [[ -n "$command_dir" ]]; then
  args+=(--command-dir "$command_dir")
fi

if [[ "${HODEXCTL_NO_PATH_UPDATE:-0}" == "1" ]]; then
  args+=(--no-path-update)
fi

if [[ -n "${GITHUB_TOKEN:-}" ]]; then
  args+=(--github-token "$GITHUB_TOKEN")
fi

"$controller_path" "${args[@]}"

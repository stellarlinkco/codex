#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONTROLLER_PATH="$SCRIPT_DIR/hodexctl.sh"

log_step() {
  printf '==> %s\n' "$1"
}

die() {
  printf '错误: %s\n' "$1" >&2
  exit 1
}

stop_background_process() {
  local pid="${1:-}"
  [[ -n "$pid" ]] || return 0
  kill "$pid" >/dev/null 2>&1 || true
  wait "$pid" 2>/dev/null || true
}

assert_contains() {
  local file_path="$1"
  local expected="$2"
  grep -F -- "$expected" "$file_path" >/dev/null 2>&1 || die "未在 ${file_path} 中找到预期内容: ${expected}"
}

assert_not_exists() {
  local file_path="$1"
  [[ ! -e "$file_path" ]] || die "不应存在: ${file_path}"
}

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
original_home="$HOME"

help_output="$tmp_dir/help.txt"
source_help_output="$tmp_dir/source-help.txt"
source_install_help_output="$tmp_dir/source-install-help.txt"
list_help_output="$tmp_dir/list-help.txt"
status_output="$tmp_dir/status.txt"
source_status_output="$tmp_dir/source-status.txt"
source_list_output="$tmp_dir/source-list.txt"
list_output="$tmp_dir/list.txt"
release_summary_output="$tmp_dir/release-summary.txt"
release_summary_args="$tmp_dir/release-summary-args.txt"
release_summary_prompt="$tmp_dir/release-summary-prompt.txt"
release_summary_fallback_output="$tmp_dir/release-summary-fallback.txt"
release_summary_fallback_args="$tmp_dir/release-summary-fallback-args.txt"
choice_stdout="$tmp_dir/choice-stdout.txt"
choice_stderr="$tmp_dir/choice-stderr.txt"
nojson_status_output="$tmp_dir/nojson-status.txt"
activate_error_output="$tmp_dir/activate-error.txt"
release_install_output="$tmp_dir/release-install.txt"
release_uninstall_output="$tmp_dir/release-uninstall.txt"
download_summary_output="$tmp_dir/download-summary.txt"
gh_fallback_output="$tmp_dir/gh-fallback.txt"
gh_missing_output="$tmp_dir/gh-missing.txt"
gh_auth_output="$tmp_dir/gh-auth.txt"
source_install_output="$tmp_dir/source-install.txt"
source_status_after_install_output="$tmp_dir/source-status-after-install.txt"
source_update_output="$tmp_dir/source-update.txt"
source_rebuild_output="$tmp_dir/source-rebuild.txt"
source_ref_candidates_output="$tmp_dir/source-ref-candidates.txt"
path_targets_output="$tmp_dir/path-targets.txt"
source_uninstall_output="$tmp_dir/source-uninstall.txt"
state_dir="$tmp_dir/state"
command_dir="$tmp_dir/commands"
release_state_dir="$tmp_dir/release-state"
release_command_dir="$tmp_dir/release-commands"
source_checkout_dir="$tmp_dir/source-checkout"
source_repo_dir="$tmp_dir/source-repo"
source_home_dir="$tmp_dir/source-home"
source_profile_file="$source_home_dir/.zshrc"
source_bin="$tmp_dir/source-bin"
release_server_root="$tmp_dir/release-server"
release_server_pid=""
ghbin="$tmp_dir/gh-bin"

log_step "检查 Bash 语法"
bash -n "$CONTROLLER_PATH"

log_step "检查空参数帮助输出"
"$CONTROLLER_PATH" >"$help_output"
assert_contains "$help_output" "用法:"
assert_contains "$help_output" "hodexctl list"
assert_contains "$help_output" "./hodexctl.sh install"

log_step "检查源码模式帮助输出"
"$CONTROLLER_PATH" source help >"$source_help_output"
assert_contains "$source_help_output" "源码模式用法:"
assert_contains "$source_help_output" "install                下载源码并准备工具链（不接管 hodex）"
assert_contains "$source_help_output" "指定源码记录名（工作区标识），默认 codex-source"

log_step "检查 source 子命令 help 语义"
"$CONTROLLER_PATH" source install --help >"$source_install_help_output"
"$CONTROLLER_PATH" list --help >"$list_help_output"
assert_contains "$source_install_help_output" "源码模式用法:"
assert_contains "$list_help_output" "版本列表用法:"
assert_contains "$list_help_output" "更新日志页操作:"

log_step "检查 zsh PATH 目标选择"
CONTROLLER_PATH_ENV="$CONTROLLER_PATH" HOME="$tmp_dir/home-path-test" SHELL="/bin/zsh" \
bash -lc '
  set -euo pipefail
  mkdir -p "$HOME"
  export HODEXCTL_SKIP_MAIN=1
  source "$CONTROLLER_PATH_ENV"
  path_profile_targets "$(select_profile_file)"
' >"$path_targets_output"
assert_contains "$path_targets_output" "$tmp_dir/home-path-test/.zprofile"
assert_contains "$path_targets_output" "$tmp_dir/home-path-test/.zshrc"

current_platform_asset="$(
  CONTROLLER_PATH_ENV="$CONTROLLER_PATH" bash -lc '
    set -euo pipefail
    export HODEXCTL_SKIP_MAIN=1
    source "$CONTROLLER_PATH_ENV"
    detect_platform
    get_asset_candidates | head -n 1
  '
)"
[[ -n "$current_platform_asset" ]] || die "未能解析当前平台 release 资产名"

log_step "检查 WSL 检测辅助逻辑"
printf 'Linux version 6.6.0-microsoft-standard-WSL2\n' >"$tmp_dir/proc-version-wsl"
CONTROLLER_PATH_ENV="$CONTROLLER_PATH" HODEXCTL_TEST_PROC_VERSION_FILE="$tmp_dir/proc-version-wsl" \
bash -lc '
  set -euo pipefail
  export HODEXCTL_SKIP_MAIN=1
  source "$CONTROLLER_PATH_ENV"
  if is_wsl_platform; then
    printf "WSL\n"
  else
    printf "NOPE\n"
  fi
' >"$tmp_dir/wsl-detect.txt"
assert_contains "$tmp_dir/wsl-detect.txt" "WSL"

log_step "检查 Linux 资产候选优先 musl"
CONTROLLER_PATH_ENV="$CONTROLLER_PATH" \
bash -lc '
  set -euo pipefail
  export HODEXCTL_SKIP_MAIN=1
  source "$CONTROLLER_PATH_ENV"
  OS_NAME="linux"
  ARCH_NAME="x86_64"
  get_asset_candidates
' >"$tmp_dir/linux-candidates.txt"
assert_contains "$tmp_dir/linux-candidates.txt" "codex-x86_64-unknown-linux-musl"
assert_contains "$tmp_dir/linux-candidates.txt" "codex-x86_64-unknown-linux-gnu"

log_step "检查未安装状态输出"
"$CONTROLLER_PATH" status --state-dir "$state_dir" >"$status_output"
assert_contains "$status_output" "正式版安装状态: 未安装"
assert_contains "$status_output" "状态目录: $state_dir"

log_step "检查源码空状态输出"
"$CONTROLLER_PATH" source status --state-dir "$state_dir" >"$source_status_output"
"$CONTROLLER_PATH" source list --state-dir "$state_dir" >"$source_list_output"
assert_contains "$source_status_output" "未安装任何源码条目"
assert_contains "$source_list_output" "当前没有已记录的源码条目"

log_step "检查 list 顶部源码入口"
listbin="$tmp_dir/list-bin"
mkdir -p "$listbin"
for cmd in bash basename dirname mktemp chmod mkdir cp install awk grep date sleep uname sed head wc tr cat rm mv tput shasum sha256sum openssl git perl less python3 jq; do
  if command -v "$cmd" >/dev/null 2>&1; then
    ln -sf "$(command -v "$cmd")" "$listbin/$cmd"
  fi
done
cat >"$listbin/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
output=""
write_format=""
url=""
while (($# > 0)); do
  case "$1" in
    -o)
      output="$2"
      shift 2
      ;;
    -w)
      write_format="$2"
      shift 2
      ;;
    http*)
      url="$1"
      shift
      ;;
    *)
      shift
      ;;
  esac
done

if [[ -z "$output" || -z "$url" ]]; then
  exit 1
fi

case "$url" in
  *"/releases?per_page=100&page=1")
    cat >"$output" <<JSON
[
  {
    "tag_name": "v9.9.9",
    "name": "9.9.9",
    "published_at": "2026-03-08T00:00:00Z",
    "html_url": "https://example.invalid/releases/v9.9.9",
    "body": "smoke",
    "assets": [
      {
        "name": "${CURRENT_PLATFORM_ASSET}",
        "browser_download_url": "https://example.invalid/download/${CURRENT_PLATFORM_ASSET}",
        "digest": ""
      }
    ]
  }
]
JSON
    ;;
  *"/releases?per_page=100&page="*)
    printf '[]\n' >"$output"
    ;;
  *)
    exit 1
    ;;
esac

if [[ "$write_format" == *"%{http_code}"* ]]; then
  printf '200'
fi
EOF
chmod +x "$listbin/curl"
PATH="$listbin" CURRENT_PLATFORM_ASSET="$current_platform_asset" "$CONTROLLER_PATH" list --state-dir "$state_dir" >"$list_output"
assert_contains "$list_output" "0. 源码模式"

log_step "检查 Bash 下载完成摘要"
downloadbin="$tmp_dir/download-bin"
mkdir -p "$downloadbin"
for cmd in bash basename dirname mktemp chmod mkdir cp install awk grep date sleep uname sed head wc tr cat rm mv tput shasum sha256sum openssl git perl less python3 jq; do
  if command -v "$cmd" >/dev/null 2>&1; then
    ln -sf "$(command -v "$cmd")" "$downloadbin/$cmd"
  fi
done
cat >"$downloadbin/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
output=""
write_out=""
range_mode=0
while (($# > 0)); do
  case "$1" in
    -o)
      output="$2"
      shift 2
      ;;
    -w)
      write_out="$2"
      shift 2
      ;;
    --range)
      range_mode=1
      shift 2
      ;;
    --progress-bar|-fL|-sS|--fail|-L)
      shift
      ;;
    http*)
      shift
      ;;
    *)
      shift
      ;;
  esac
done
if ((range_mode)); then
  exit 0
fi
[[ -n "$output" ]] || exit 0
printf 'fake-binary' >"$output"
if [[ -n "$write_out" ]]; then
  printf '1048576\t524288\t2.0\n'
fi
EOF
chmod +x "$downloadbin/curl"
PATH="$downloadbin" HODEX_RELEASE_BASE_URL="https://example.invalid/releases" "$CONTROLLER_PATH" download latest --download-dir "$tmp_dir/downloads" >"$download_summary_output"
assert_contains "$download_summary_output" "下载完成:"
assert_contains "$download_summary_output" "平均速度"

log_step "检查 Bash 候选输入提示不会污染返回值"
CONTROLLER_PATH_ENV="$CONTROLLER_PATH" \
bash -lc '
  set -euo pipefail
  export HODEXCTL_SKIP_MAIN=1
  source "$CONTROLLER_PATH_ENV"
  reset_choice_candidates
  append_choice_candidate "alpha"
  append_choice_candidate "beta"
  printf "2\n" | prompt_value_with_choice_candidates "测试字段" "default" "测试备注"
' >"$choice_stdout" 2>"$choice_stderr"
assert_contains "$choice_stdout" "beta"
assert_contains "$choice_stderr" "测试字段"
assert_contains "$choice_stderr" "可选项:"

log_step "检查 release changelog 总结优先调用 hodex"
summary_bin="$tmp_dir/summary-bin"
mkdir -p "$summary_bin"
cat >"$summary_bin/hodex" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "exec" && "${2:-}" == "--help" ]]; then
  exit 0
fi
printf '%s\n' "$*" >"$TRACE_ARGS_FILE"
cat >"$TRACE_PROMPT_FILE"
printf '%s\n' '{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"这是 hodex 总结结果"}}'
EOF
chmod +x "$summary_bin/hodex"
cat >"$summary_bin/codex" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "不应调用 codex" >&2
exit 1
EOF
chmod +x "$summary_bin/codex"
CONTROLLER_PATH_ENV="$CONTROLLER_PATH" \
TRACE_ARGS_FILE="$release_summary_args" \
TRACE_PROMPT_FILE="$release_summary_prompt" \
SUMMARY_OUTPUT_FILE="$release_summary_output" \
SUMMARY_BIN_DIR="$summary_bin" \
CURRENT_PLATFORM_ASSET="$current_platform_asset" \
bash -lc '
  set -euo pipefail
  export HODEXCTL_SKIP_MAIN=1
  export PATH="$SUMMARY_BIN_DIR:$PATH"
  source "$CONTROLLER_PATH_ENV"
  detect_platform
  init_color_theme
  init_json_backend_if_available
  release_file="$(mktemp)"
  cat >"$release_file" <<JSON
{
  "tag_name": "v1.2.3",
  "name": "1.2.3",
  "published_at": "2026-03-09T00:00:00Z",
  "html_url": "https://example.invalid/releases/v1.2.3",
  "body": "- add feature A\n- fix bug B",
  "assets": [
    {
      "name": "${CURRENT_PLATFORM_ASSET}",
      "browser_download_url": "https://example.invalid/download/${CURRENT_PLATFORM_ASSET}",
      "digest": ""
    }
  ]
}
JSON
  summarize_release_changelog "$release_file" "1.2.3" >"$SUMMARY_OUTPUT_FILE" 2>&1
'
assert_contains "$release_summary_output" "这是 hodex 总结结果"
assert_contains "$release_summary_args" "exec --skip-git-repo-check --color never --json -"
assert_contains "$release_summary_prompt" "版本: 1.2.3"
assert_contains "$release_summary_prompt" "完整 changelog:"
assert_contains "$release_summary_prompt" "新增功能"
assert_contains "$release_summary_prompt" "修复内容"
assert_contains "$release_summary_prompt" "破坏性变更 / 迁移要求"
assert_contains "$release_summary_prompt" "- add feature A"

log_step "检查 release changelog 总结会回退到 codex"
fallback_bin="$tmp_dir/summary-fallback-bin"
mkdir -p "$fallback_bin"
cat >"$fallback_bin/hodex" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "exec" && "${2:-}" == "--help" ]]; then
  exit 1
fi
exit 1
EOF
chmod +x "$fallback_bin/hodex"
cat >"$fallback_bin/codex" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "exec" && "${2:-}" == "--help" ]]; then
  exit 0
fi
printf '%s\n' "$*" >"$TRACE_ARGS_FILE"
cat >/dev/null
printf '%s\n' '{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"这是 codex 回退总结结果"}}'
EOF
chmod +x "$fallback_bin/codex"
CONTROLLER_PATH_ENV="$CONTROLLER_PATH" \
TRACE_ARGS_FILE="$release_summary_fallback_args" \
SUMMARY_OUTPUT_FILE="$release_summary_fallback_output" \
SUMMARY_BIN_DIR="$fallback_bin" \
CURRENT_PLATFORM_ASSET="$current_platform_asset" \
bash -lc '
  set -euo pipefail
  export HODEXCTL_SKIP_MAIN=1
  export PATH="$SUMMARY_BIN_DIR:$PATH"
  source "$CONTROLLER_PATH_ENV"
  detect_platform
  init_color_theme
  init_json_backend_if_available
  release_file="$(mktemp)"
  cat >"$release_file" <<JSON
{
  "tag_name": "v2.0.0",
  "name": "2.0.0",
  "published_at": "2026-03-09T00:00:00Z",
  "html_url": "https://example.invalid/releases/v2.0.0",
  "body": "fallback smoke",
  "assets": [
    {
      "name": "${CURRENT_PLATFORM_ASSET}",
      "browser_download_url": "https://example.invalid/download/${CURRENT_PLATFORM_ASSET}",
      "digest": ""
    }
  ]
}
JSON
  summarize_release_changelog "$release_file" "2.0.0" >"$SUMMARY_OUTPUT_FILE" 2>&1
'
assert_contains "$release_summary_fallback_output" "这是 codex 回退总结结果"
assert_contains "$release_summary_fallback_output" "已自动改用 codex"
assert_contains "$release_summary_fallback_args" "exec --skip-git-repo-check --color never --json -"

log_step "检查 GitHub API 403 时自动回退 gh"
mkdir -p "$ghbin"
for cmd in bash basename dirname mktemp chmod mkdir cp install awk grep date sleep uname sed head wc tr cat rm mv tput shasum sha256sum openssl git perl less python3 jq; do
  if command -v "$cmd" >/dev/null 2>&1; then
    ln -sf "$(command -v "$cmd")" "$ghbin/$cmd"
  fi
done
cat >"$ghbin/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
output=""
write_format=""
while (($# > 0)); do
  case "$1" in
    -o) output="$2"; shift 2 ;;
    -w) write_format="$2"; shift 2 ;;
    *) shift ;;
  esac
done
printf '{"message":"rate limited"}\n' >"$output"
if [[ "$write_format" == *"%{http_code}"* ]]; then
  printf '403'
fi
EOF
chmod +x "$ghbin/curl"
cat >"$ghbin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "api" ]]; then
  cat <<JSON
[
  {
    "tag_name": "v9.9.8",
    "name": "9.9.8",
    "published_at": "2026-03-07T00:00:00Z",
    "html_url": "https://example.invalid/releases/v9.9.8",
    "body": "gh fallback",
    "assets": [
      {
        "name": "${CURRENT_PLATFORM_ASSET}",
        "browser_download_url": "https://example.invalid/download/${CURRENT_PLATFORM_ASSET}",
        "digest": ""
      }
    ]
  }
]
JSON
  exit 0
fi
exit 1
EOF
chmod +x "$ghbin/gh"
PATH="$ghbin" CURRENT_PLATFORM_ASSET="$current_platform_asset" "$CONTROLLER_PATH" list --state-dir "$state_dir" >"$gh_fallback_output"
assert_contains "$gh_fallback_output" "0. 源码模式"
assert_contains "$gh_fallback_output" "已自动改用 gh api 获取 GitHub 数据。"

log_step "检查 GitHub API 403 且 gh 不可用时的提示"
mkdir -p "$tmp_dir/gh-missing-bin"
for cmd in bash basename dirname mktemp chmod mkdir cp install awk grep date sleep uname sed head wc tr cat rm mv tput shasum sha256sum openssl git perl less python3 jq; do
  if command -v "$cmd" >/dev/null 2>&1; then
    ln -sf "$(command -v "$cmd")" "$tmp_dir/gh-missing-bin/$cmd"
  fi
done
cat >"$tmp_dir/gh-missing-bin/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
output=""
write_format=""
while (($# > 0)); do
  case "$1" in
    -o) output="$2"; shift 2 ;;
    -w) write_format="$2"; shift 2 ;;
    *) shift ;;
  esac
done
printf '{"message":"rate limited"}\n' >"$output"
if [[ "$write_format" == *"%{http_code}"* ]]; then
  printf '403'
fi
EOF
chmod +x "$tmp_dir/gh-missing-bin/curl"
if PATH="$tmp_dir/gh-missing-bin" "$CONTROLLER_PATH" list --state-dir "$state_dir" >"$gh_missing_output" 2>&1; then
  die "gh 缺失时的 403 场景不应成功"
fi
assert_contains "$gh_missing_output" "当前未检测到 gh"

log_step "检查 GitHub API 403 且 gh 未登录时的提示"
mkdir -p "$tmp_dir/gh-auth-bin"
for cmd in bash basename dirname mktemp chmod mkdir cp install awk grep date sleep uname sed head wc tr cat rm mv tput shasum sha256sum openssl git perl less python3 jq; do
  if command -v "$cmd" >/dev/null 2>&1; then
    ln -sf "$(command -v "$cmd")" "$tmp_dir/gh-auth-bin/$cmd"
  fi
done
cp "$tmp_dir/gh-missing-bin/curl" "$tmp_dir/gh-auth-bin/curl"
cat >"$tmp_dir/gh-auth-bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "not logged in to any hosts" >&2
exit 1
EOF
chmod +x "$tmp_dir/gh-auth-bin/gh"
if PATH="$tmp_dir/gh-auth-bin" "$CONTROLLER_PATH" list --state-dir "$state_dir" >"$gh_auth_output" 2>&1; then
  die "gh 未登录时的 403 场景不应成功"
fi
assert_contains "$gh_auth_output" "gh 未登录"
assert_contains "$gh_auth_output" "gh auth login"

log_step "检查无 python3/jq 时 release-only 状态输出"
tmpbin="$tmp_dir/minimal-bin"
mkdir -p "$tmpbin"
for cmd in bash basename dirname curl mktemp chmod mkdir cp install awk grep date sleep uname sed head wc tr cat rm mv tput shasum sha256sum openssl git perl less; do
  if command -v "$cmd" >/dev/null 2>&1; then
    ln -sf "$(command -v "$cmd")" "$tmpbin/$cmd"
  fi
done
PATH="$tmpbin" "$CONTROLLER_PATH" status --state-dir "$state_dir" >"$nojson_status_output"
assert_contains "$nojson_status_output" "正式版安装状态: 未安装"

log_step "检查源码模式拒绝接管 hodex"
if "$CONTROLLER_PATH" source install --activate --state-dir "$state_dir" >"$activate_error_output" 2>&1; then
  die "源码模式不应接受 --activate"
fi
assert_contains "$activate_error_output" "源码模式不允许接管 hodex"

log_step "检查源码菜单新交互文案"
assert_contains "$list_output" "源码下载 / 管理"
assert_contains "$source_help_output" "指定源码记录名（工作区标识），默认 codex-source"

log_step "检查 release-only 安装与卸载清理"
mkdir -p "$release_server_root/latest/download"
cat >"$release_server_root/latest/download/${current_platform_asset}" <<'EOF'
#!/usr/bin/env bash
if [[ "${1:-}" == "--version" ]]; then
  echo "codex-cli 9.9.9"
  exit 0
fi
echo "dummy"
EOF
chmod +x "$release_server_root/latest/download/${current_platform_asset}"
release_port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
python3 -m http.server "$release_port" --bind 127.0.0.1 --directory "$release_server_root" >/dev/null 2>&1 &
release_server_pid=$!
trap 'stop_background_process "${release_server_pid:-}"; rm -rf "$tmp_dir"' EXIT
for _ in {1..50}; do
  if curl -fsS "http://127.0.0.1:$release_port/" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done
HODEX_RELEASE_BASE_URL="http://127.0.0.1:$release_port" "$CONTROLLER_PATH" install \
  --yes \
  --no-path-update \
  --state-dir "$release_state_dir" \
  --command-dir "$release_command_dir" >"$release_install_output" 2>&1
assert_contains "$release_install_output" "安装完成"
test -x "$release_command_dir/hodex"
test -x "$release_command_dir/hodexctl"
HODEX_RELEASE_BASE_URL="http://127.0.0.1:$release_port" "$CONTROLLER_PATH" uninstall \
  --state-dir "$release_state_dir" >"$release_uninstall_output" 2>&1
assert_contains "$release_uninstall_output" "已删除正式版二进制、包装器和安装状态。"
assert_not_exists "$release_command_dir/hodex"
assert_not_exists "$release_command_dir/hodexctl"
assert_not_exists "$release_state_dir/state.json"
stop_background_process "$release_server_pid"
release_server_pid=""
trap 'rm -rf "$tmp_dir"' EXIT

log_step "检查源码模式本地闭环同步"
if ! command -v git >/dev/null 2>&1 || ! command -v cargo >/dev/null 2>&1 || ! command -v rustc >/dev/null 2>&1 || { ! command -v python3 >/dev/null 2>&1 && ! command -v jq >/dev/null 2>&1; }; then
  log_step "环境缺少 git/cargo/rustc/python3|jq，跳过源码闭环集成测试"
else
  mkdir -p "$source_repo_dir/src" "$command_dir" "$source_home_dir"
  mkdir -p "$source_bin"
  : >"$source_profile_file"
  if command -v just >/dev/null 2>&1; then
    ln -sf "$(command -v just)" "$source_bin/just"
  else
    cat >"$source_bin/just" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
    chmod +x "$source_bin/just"
  fi
  cat >"$source_repo_dir/Cargo.toml" <<'EOF'
[package]
name = "codex-cli"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "codex"
path = "src/main.rs"
EOF
  cat >"$source_repo_dir/src/main.rs" <<'EOF'
fn main() {
    println!("smoke-build 0.1.0");
}
EOF

  git -C "$source_repo_dir" init -b main >/dev/null
  git -C "$source_repo_dir" config user.name "hodexctl-smoke" >/dev/null
  git -C "$source_repo_dir" config user.email "hodexctl-smoke@example.com" >/dev/null
  git -C "$source_repo_dir" add Cargo.toml src/main.rs >/dev/null
  git -C "$source_repo_dir" commit -m "init smoke repo" >/dev/null
  git -C "$source_repo_dir" tag smoke-tag >/dev/null

  PATH="$source_bin:$PATH" HOME="$source_home_dir" SHELL="/bin/zsh" RUSTUP_HOME="${RUSTUP_HOME:-$original_home/.rustup}" CARGO_HOME="${CARGO_HOME:-$original_home/.cargo}" "$CONTROLLER_PATH" source install \
    --yes \
    --state-dir "$state_dir" \
    --command-dir "$command_dir" \
    --git-url "$source_repo_dir" \
    --profile smoke-source \
    --ref main \
    --checkout-dir "$source_checkout_dir" >"$source_install_output" 2>&1
  assert_contains "$source_install_output" "结果摘要"
  assert_contains "$source_install_output" "源码记录名: smoke-source"
  assert_contains "$source_install_output" "当前 ref: main"
  test -d "$source_checkout_dir/.git"
  test -x "$command_dir/hodexctl"
  assert_contains "$source_profile_file" "# >>> hodexctl >>>"
  install_head="$(git -C "$source_checkout_dir" rev-parse HEAD)"
  repo_head="$(git -C "$source_repo_dir" rev-parse HEAD)"
  [[ "$install_head" == "$repo_head" ]] || die "源码安装后 checkout HEAD 不一致"

  PATH="$source_bin:$PATH" HOME="$source_home_dir" SHELL="/bin/zsh" RUSTUP_HOME="${RUSTUP_HOME:-$original_home/.rustup}" CARGO_HOME="${CARGO_HOME:-$original_home/.cargo}" "$CONTROLLER_PATH" source status \
    --yes \
    --state-dir "$state_dir" \
    --command-dir "$command_dir" >"$source_status_after_install_output" 2>&1
  assert_contains "$source_status_after_install_output" "名称: smoke-source"
  assert_contains "$source_status_after_install_output" "模式: 仅管理源码 checkout 与工具链，不生成源码命令入口"

  cat >"$source_repo_dir/src/main.rs" <<'EOF'
fn main() {
    println!("smoke-build 0.2.0");
}
EOF
  git -C "$source_repo_dir" add src/main.rs >/dev/null
  git -C "$source_repo_dir" commit -m "update smoke repo" >/dev/null

  PATH="$source_bin:$PATH" HOME="$source_home_dir" SHELL="/bin/zsh" RUSTUP_HOME="${RUSTUP_HOME:-$original_home/.rustup}" CARGO_HOME="${CARGO_HOME:-$original_home/.cargo}" "$CONTROLLER_PATH" source update \
    --yes \
    --state-dir "$state_dir" \
    --command-dir "$command_dir" >"$source_update_output" 2>&1
  assert_contains "$source_update_output" "更新源码"
  update_head="$(git -C "$source_checkout_dir" rev-parse HEAD)"
  repo_head="$(git -C "$source_repo_dir" rev-parse HEAD)"
  [[ "$update_head" == "$repo_head" ]] || die "源码更新后 checkout HEAD 不一致"

  git -C "$source_repo_dir" checkout -b feature-smoke-switch >/dev/null
  CONTROLLER_PATH_ENV="$CONTROLLER_PATH" STATE_FILE_ENV="$state_dir/state.json" REPO_ENV="$source_repo_dir" PROFILE_ENV="smoke-source" CHECKOUT_ENV="$source_checkout_dir" \
    bash -lc '
      set -euo pipefail
      export HODEXCTL_SKIP_MAIN=1
      source "$CONTROLLER_PATH_ENV"
      init_json_backend_if_available
      emit_source_ref_candidates "$REPO_ENV" "$STATE_FILE_ENV" "$PROFILE_ENV" "$CHECKOUT_ENV"
    ' >"$source_ref_candidates_output"
  assert_contains "$source_ref_candidates_output" "feature-smoke-switch"
  if grep -F -- "smoke-tag" "$source_ref_candidates_output" >/dev/null 2>&1; then
    die "branch 候选列表不应默认混入 tag"
  fi

  PATH="$source_bin:$PATH" HOME="$source_home_dir" SHELL="/bin/zsh" RUSTUP_HOME="${RUSTUP_HOME:-$original_home/.rustup}" CARGO_HOME="${CARGO_HOME:-$original_home/.cargo}" "$CONTROLLER_PATH" source switch \
    --yes \
    --state-dir "$state_dir" \
    --command-dir "$command_dir" \
    --ref feature-smoke-switch >"$source_rebuild_output" 2>&1
  assert_contains "$source_rebuild_output" "切换 ref 并同步源码"
  switch_head="$(git -C "$source_checkout_dir" rev-parse --abbrev-ref HEAD)"
  [[ "$switch_head" == "feature-smoke-switch" ]] || die "源码切换 ref 后分支不正确"

  if PATH="$source_bin:$PATH" HOME="$source_home_dir" SHELL="/bin/zsh" RUSTUP_HOME="${RUSTUP_HOME:-$original_home/.rustup}" CARGO_HOME="${CARGO_HOME:-$original_home/.cargo}" "$CONTROLLER_PATH" source rebuild \
    --yes \
    --state-dir "$state_dir" \
    --command-dir "$command_dir" >"$source_rebuild_output" 2>&1; then
    die "source rebuild 已移除，不应成功"
  fi
  assert_contains "$source_rebuild_output" "source rebuild 已移除"

  PATH="$source_bin:$PATH" HOME="$source_home_dir" SHELL="/bin/zsh" RUSTUP_HOME="${RUSTUP_HOME:-$original_home/.rustup}" CARGO_HOME="${CARGO_HOME:-$original_home/.cargo}" "$CONTROLLER_PATH" source uninstall \
    --yes \
    --keep-checkout \
    --state-dir "$state_dir" \
    --command-dir "$command_dir" >"$source_uninstall_output" 2>&1
  assert_contains "$source_uninstall_output" "卸载源码条目"
  test -d "$source_checkout_dir"
  assert_not_exists "$command_dir/smoke-source"
  assert_not_exists "$command_dir/hodexctl"
  if grep -F "# >>> hodexctl >>>" "$source_profile_file" >/dev/null 2>&1; then
    die "源码 profile 全部卸载后不应保留 PATH block"
  fi
fi

log_step "Smoke 测试通过"

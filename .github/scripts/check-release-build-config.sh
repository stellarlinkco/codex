#!/usr/bin/env bash
set -euo pipefail

required_vars=(
  CARGO_PROFILE_RELEASE_LTO
  CARGO_PROFILE_RELEASE_CODEGEN_UNITS
  CARGO_PROFILE_RELEASE_DEBUG
  CARGO_PROFILE_RELEASE_STRIP
)

make_output="$(make -n release-codex)"
for var in "${required_vars[@]}"; do
  if ! grep -q -- "-u ${var}" <<<"${make_output}"; then
    echo "make release-codex 缺少变量清理: ${var}" >&2
    exit 1
  fi
done
if ! grep -q -- "cargo build -p codex-cli --bin codex --release" <<<"${make_output}"; then
  echo "make release-codex 未调用预期 cargo release 构建命令" >&2
  exit 1
fi

just_target_body="$(awk '
  /^release-codex out=/ { in_target=1; next }
  in_target && /^[^[:space:]]/ { exit }
  in_target { print }
' justfile)"

if [[ -z "${just_target_body}" ]]; then
  echo "justfile 缺少 release-codex 目标" >&2
  exit 1
fi

for var in "${required_vars[@]}"; do
  if ! grep -q -- "-u ${var}" <<<"${just_target_body}"; then
    echo "just release-codex 缺少变量清理: ${var}" >&2
    exit 1
  fi
done
if ! grep -q -- "cargo build -p codex-cli --bin codex --release" <<<"${just_target_body}"; then
  echo "just release-codex 未调用预期 cargo release 构建命令" >&2
  exit 1
fi

echo "release 构建入口校验通过"

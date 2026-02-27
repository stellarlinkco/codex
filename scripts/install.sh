#!/usr/bin/env bash
set -euo pipefail

repo="${CODEX_REPO:-stellarlinkco/codex}"
install_dir="${INSTALL_DIR:-"$HOME/.local/bin"}"
bin_name="codex"

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Darwin)
    case "$arch" in
      arm64) candidates=(aarch64-apple-darwin) ;;
      x86_64) candidates=(x86_64-apple-darwin) ;;
      *) echo "Unsupported macOS architecture: $arch" >&2; exit 1 ;;
    esac
    ;;
  Linux)
    case "$arch" in
      aarch64 | arm64) candidates=(aarch64-unknown-linux-musl aarch64-unknown-linux-gnu) ;;
      x86_64) candidates=(x86_64-unknown-linux-musl x86_64-unknown-linux-gnu) ;;
      *) echo "Unsupported Linux architecture: $arch" >&2; exit 1 ;;
    esac
    ;;
  *)
    echo "Unsupported OS: $os" >&2
    exit 1
    ;;
esac

if ! command -v curl >/dev/null 2>&1; then
  echo "Missing dependency: curl" >&2
  exit 1
fi

base_url="${CODEX_BASE_URL:-https://github.com/${repo}/releases/latest/download}"
base_url="${base_url%/}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

asset=""
for target in "${candidates[@]}"; do
  candidate="codex-${target}"
  url="${base_url}/${candidate}"
  if curl -fsSL --range 0-0 "$url" >/dev/null 2>&1; then
    asset="$candidate"
    break
  fi
done

if [[ -z "$asset" ]]; then
  echo "No matching release asset found for OS=$os ARCH=$arch in $repo" >&2
  exit 1
fi

mkdir -p "$install_dir"
curl -fsSL "${base_url}/${asset}" -o "${tmp}/${asset}"
chmod +x "${tmp}/${asset}"
install -m 755 "${tmp}/${asset}" "${install_dir}/${bin_name}"

echo "Installed ${bin_name} to ${install_dir}/${bin_name}"
echo "Ensure ${install_dir} is on your PATH."

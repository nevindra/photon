#!/usr/bin/env bash
# Install process-compose — the dev process runner used by `make dev`.
# Downloads the release binary into ~/.local/bin (override with PC_DEST).
set -euo pipefail

VERSION="${PC_VERSION:-v1.116.0}"
DEST="${PC_DEST:-$HOME/.local/bin}"

os="$(uname -s | tr '[:upper:]' '[:lower:]')"
case "$(uname -m)" in
  x86_64 | amd64) arch="amd64" ;;
  aarch64 | arm64) arch="arm64" ;;
  *) echo "unsupported arch: $(uname -m)" >&2; exit 1 ;;
esac

asset="process-compose_${os}_${arch}.tar.gz"
url="https://github.com/F1bonacc1/process-compose/releases/download/${VERSION}/${asset}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo ">> downloading $url"
curl -fL --retry 3 -o "$tmp/pc.tar.gz" "$url"
tar -xzf "$tmp/pc.tar.gz" -C "$tmp" process-compose
mkdir -p "$DEST"
install -m 0755 "$tmp/process-compose" "$DEST/process-compose"
echo ">> installed process-compose $VERSION to $DEST/process-compose"

if ! command -v process-compose >/dev/null 2>&1; then
  echo ">> NOTE: $DEST is not on your PATH — add it, e.g. export PATH=\"$DEST:\$PATH\""
fi

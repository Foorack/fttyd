#!/usr/bin/env bash
# Builds a fully static binary: no libc at runtime, so it runs on any Linux including
# alpine and scratch images.
#
# The only non-Rust dependency is a C compiler that targets musl, because ring
# (rustls's crypto backend) assembles C. On a musl-native container that comes for free.
#
# Host alternative, if you would rather not use docker:
#     sudo apt install musl-tools
#     rustup target add x86_64-unknown-linux-musl
#     CC_x86_64_unknown_linux_musl=musl-gcc \
#       cargo build --release --target x86_64-unknown-linux-musl
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
image="${FTTYD_STATIC_IMAGE:-rust:1-alpine}"
out="${1:-$repo/target/static}"

# build.rs embeds web/dist, so the frontend has to exist before cargo runs.
if [[ ! -f "$repo/web/dist/index.html" ]]; then
  echo "==> building the frontend"
  (cd "$repo/web" && bun install --frozen-lockfile && bun run build)
fi

# The source is mounted read-only: cargo must not rewrite the lockfile or the working
# tree from inside the container. Failing here means the lockfile is out of date.
echo "==> checking the lockfile"
cargo metadata --locked --format-version 1 --manifest-path "$repo/Cargo.toml" >/dev/null

echo "==> building in $image"
mkdir -p "$out"
docker run --rm \
  -v "$repo":/src:ro \
  -v "$out":/out \
  -w /src \
  "$image" sh -c '
    set -e
    apk add --no-cache musl-dev gcc >/dev/null
    CARGO_TARGET_DIR=/target cargo build --release --locked
    cp /target/release/fttyd /out/fttyd
  '

echo "==> $out/fttyd"
file "$out/fttyd"

#!/usr/bin/env bash
# Builds a fully static binary: no libc at runtime, so it runs on any Linux including
# alpine and scratch images.
#
# The only non-Rust dependency is a C compiler that targets musl, because ring
# (rustls's crypto backend) assembles C. On Debian/Ubuntu `apt install musl-tools`
# provides x86_64-linux-musl-gcc, which cc-rs finds on its own. Without a host
# toolchain this falls back to a musl-native container.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="x86_64-unknown-linux-musl"
image="${FTTYD_STATIC_IMAGE:-rust:1-alpine}"
out="${1:-$repo/target/static/fttyd}"

# build.rs embeds web/dist, so the frontend has to exist before cargo runs.
if [[ ! -f "$repo/web/dist/index.html" ]]; then
  echo "==> building the frontend"
  (cd "$repo/web" && bun install --frozen-lockfile && bun run build)
fi

mkdir -p "$(dirname "$out")"

# Prefer the host toolchain: it reuses the normal target directory and is much faster.
cc=""
if command -v x86_64-linux-musl-gcc >/dev/null 2>&1; then
  cc="x86_64-linux-musl-gcc"
elif command -v musl-gcc >/dev/null 2>&1; then
  cc="musl-gcc"
fi

if [[ -n "$cc" ]]; then
  echo "==> building with the host musl toolchain ($cc)"
  if [[ "$cc" == "musl-gcc" ]]; then
    # cc-rs probes for x86_64-linux-musl-gcc by name; point it at the generic wrapper.
    export CC_x86_64_unknown_linux_musl="$cc"
  fi
  if ! rustup target list --installed | grep -qx "$target"; then
    rustup target add "$target"
  fi
  cargo build --release --locked --target "$target" --manifest-path "$repo/Cargo.toml"
  cp "$repo/target/$target/release/fttyd" "$out"
else
  echo "==> no musl toolchain on the host; building in $image"
  # Mounted read-only so cargo cannot rewrite the lockfile or the working tree.
  cargo metadata --locked --format-version 1 --manifest-path "$repo/Cargo.toml" >/dev/null
  docker run --rm \
    -v "$repo":/src:ro \
    -v "$(dirname "$out")":/out \
    -w /src \
    "$image" sh -c "
      set -e
      apk add --no-cache musl-dev gcc >/dev/null
      CARGO_TARGET_DIR=/target cargo build --release --locked
      cp /target/release/fttyd /out/$(basename "$out")
    "
fi

echo "==> $out"
file "$out"
if ! file "$out" | grep -q "statically linked\|static-pie"; then
  echo "error: $out is not statically linked" >&2
  exit 1
fi

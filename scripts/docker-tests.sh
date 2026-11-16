#!/usr/bin/env bash
# Privilege, socket and edge-case tests that need an isolated, disposable machine.
#
# These cannot run in the Playwright suite: they need root inside a container, a
# non-root user, and a filesystem the test is allowed to break.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
image="${FTTYD_TEST_IMAGE:-debian:bookworm-slim}"
binary="$repo/target/debug/fttyd"

if [[ ! -x "$binary" ]]; then
  echo "error: build the binary first (cargo build)" >&2
  exit 1
fi

echo "==> running container tests on $image"
docker run --rm \
  -v "$binary:/usr/local/bin/fttyd:ro" \
  -v "$repo/scripts/docker-checks.sh:/usr/local/bin/docker-checks.sh:ro" \
  --entrypoint /bin/bash \
  "$image" /usr/local/bin/docker-checks.sh

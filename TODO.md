# TODO

Work ledger for the implementation described in [PLAN.md](./PLAN.md). Levels: task →
subtask → sub-subtask. Checked items are verified by the tests named beside them.

## 1. Parity core

- [x] PTY child management
  - [x] Spawn via `portable-pty`: `setsid`, controlling terminal, initial winsize before exec
  - [x] `TERM` from `-T`, `TTYD_USER` from `-H`, cwd from `-w`
  - [x] Exit status capture, 128+signal mapping, close code 1000/1006 (unit: `pty::tests`)
  - [x] Input queue with backpressure, output queue that throttles the child
- [x] ttyd wire protocol
  - [x] Client bytes `0 1 2 3 {`, server bytes `0 1 2` (unit: `proto::tests`)
  - [x] `1<command> (<host>)` and `2<prefs json>` handshake order
  - [x] `{` control message carries `AuthToken` + `columns`/`rows`, spawn happens there
  - [x] `-a` URL args appended to the command (unit: `ws::tests::url_args_*`)
- [x] WebSocket + token endpoints
  - [x] `/ws` with `tty` subprotocol, per-connection child, `{AuthToken}` validation → 1008
  - [x] `/token` echoes the credential exactly like ttyd (unit: `server::tests::token_*`)
  - [x] 401 + `Basic realm="ttyd"`, 407 for `-H` (unit: `server::tests::missing_credential_is_challenged`)
  - [x] `-m`/`-o`/`-q` client accounting (unit: `state::tests`)
  - [x] Ping keepalive, hangup after `interval + 7`
- [x] Frontend
  - [x] bun + vite build, embedded and served gzip'd with ETag/304
  - [x] xterm.js + fit + webgl (DOM fallback), unicode 11, web links
  - [x] Flow control (`PAUSE`/`RESUME`), streaming UTF-8 decode, reconnect semantics
  - [x] `-t` client option passthrough with type-checked merging
- [x] Conformance gate
  - [x] Extract ttyd's shipped bundle from `src/html.h` (`scripts/extract-ttyd-frontend.py`)
  - [x] Drive it against fttyd with Playwright (`e2e/tests/conformance.spec.ts`)

## 2. Parity breadth

- [x] CLI surface: every ttyd flag, long-option prefixes, bundling, `-p8080`, `--`
  - [x] `getopt_long`-compatible parser with unit coverage (`cli::tests`)
  - [x] Documented divergences: unknown options fail, no trailing-garbage numbers, `--` implied
- [x] Networking
  - [x] `-b` base path on every endpoint, `/base` → `/base/` redirect (E2E)
  - [x] `-i` literal IP, interface name via `getifaddrs`, UNIX socket + stale-socket cleanup
  - [x] `-U user:group` socket ownership
  - [x] `-6` IPv6 bind
- [x] Privileges: `-u`/`-g` after bind, `setgroups` cleared, non-root rejected up front
- [x] TLS
  - [x] `-S`/`-C`/`-K` via rustls (no TLS 1.0/1.1 support at all)
  - [x] `-A` client certificate verification (unit: `tls::tests`, incl. rejection without a cert)
  - [x] wss works through the same listener
- [ ] `-B` browser launch covered by a test (would open a browser on the test machine)
- [x] `--help` diffed against ttyd's text (only the name, version, URL and `--file-root` differ)
- [x] `-O`/`-m`/`-o`/`-q`/`-a`/`-I` covered end to end (E2E)

## 3. Filesystem read-only

- [x] `GET /api/fs` listing
  - [x] name, size, mtime, mode, type, symlink target, per-entry errors (unit: `fs::tests`)
  - [x] normalisation, `..` and symlink escape rejection (unit: `fs::tests::roots_*`)
- [x] `GET /api/download`
  - [x] `Content-Length`, `Content-Disposition`, `Range`/`206` (unit: `api::tests::range_*`)
  - [x] refuses directories and special files
- [x] `GET /api/archive?format=zip|tar.gz`
  - [x] streamed into the body through a bounded channel, ZIP64, no temp files
  - [x] skips FIFOs/sockets/devices, no symlink following, preserves mtime/mode/uid/gid
  - [x] skipped entries are reported inside the archive (`fttyd-archive-report.txt`)
  - [x] `unzip -t` / `tar -tzf` verification in E2E
- [x] File tree UI
  - [x] lazy loading, sort, hidden toggle, sizes/mtime, symlink marking
  - [x] keyboard navigation, locked entries, row windowing for large directories

## 4. Stats

- [x] Host-wide sampler at 1 Hz, primed so the first CPU sample is valid (unit: `stats::tests`)
- [x] `GET /api/stats` SSE with subscriber fan-out
- [x] CPU / RAM / root disk gauges with numeric percentages
- [x] `1fr / 4px / sidebar` grid with the terminal at ~3/4, draggable splitter persisted
- [x] Mobile drawer below 900px (E2E)

## 5. Mutations (`-W` only)

- [x] CSRF header on every non-GET, default-on, config-free (unit + E2E)
- [x] Audit log of actor (from `-H`) + operation + path
- [x] Uploads
  - [x] `PUT /api/upload` raw body, resumable via `Content-Range` (E2E)
  - [x] `POST /api/upload` multipart for `curl -F`
  - [x] free-space check, atomic rename from a `.fttyd-part` file
  - [x] UI upload with progress, drag and drop including directories
- [x] `PATCH`/`DELETE /api/fs`: rename, mkdir, chmod, delete (E2E)
- [x] Read-only servers reject every mutation with 403 (E2E)

## 6. Sync and polish

- [x] Tree follows shell cwd (`/proc/<pid>/cwd` poll pushed over the websocket)
- [x] Pinning on manual navigation, "follow" toggle to resync (E2E)
- [x] "Open terminal here" types `cd` into the attached shell (E2E)
- [x] Mobile drawer
- [x] Numeric values beside every gauge (no colour-only encoding), visible focus outlines
- [ ] Full accessibility audit

## CI

- [x] `.github/workflows/ci.yml` on every push and pull request
  - [x] author check over the pushed range (or full history on first push/rewrite)
  - [x] `cargo fmt --check`
  - [x] `cargo clippy -D warnings` for all targets
  - [x] frontend typecheck + production build
  - [x] `cargo test --locked`
- [x] `scripts/check-authors.sh` with 15 self-checks (`scripts/test-check-authors.sh`)
- [x] Optional pre-commit guard: `.githooks/commit-msg` + `core.hooksPath`
- [ ] Playwright and docker suites in CI (need a browser download and privileges)

## Testing and delivery

- [x] Unit tests across CLI, protocol, PTY, auth, routing, state, fs, archives, uploads, stats, TLS
- [x] Playwright suite covering terminal, conformance, files, stats, upload, CSRF, follow-cwd, options
- [x] Docker tests (`debian:bookworm-slim`): `-u`/`-g`, `-i`/`-U`, permission-denied paths, `/proc`, `/dev`
- [x] README and man page
- [x] Single binary: assets embedded, `ldd` shows only libc/libm/libgcc
- [x] Fully static musl build. `ring` needs a musl-targeting C compiler; with `musl-tools`
      installed, plain `cargo build --release --target x86_64-unknown-linux-musl` works, and
      `scripts/build-static.sh` prefers that while falling back to `rust:1-alpine`. Verified
      `static-pie linked`, with HTTP, websocket/PTY and TLS exercised from an alpine container.

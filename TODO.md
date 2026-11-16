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
- [ ] TLS
  - [ ] `-S`/`-C`/`-K` via rustls, TLS 1.2 minimum
  - [ ] `-A` client certificate verification
- [ ] `-B` browser launch covered by a test
- [ ] `--help` diffed against ttyd's text

## 3. Filesystem read-only

- [ ] `GET /api/fs` listing
  - [ ] name, size, mtime, mode, type, symlink target, per-entry errors
  - [ ] canonicalisation, `..` and symlink escape rejection (unit tests)
- [ ] `GET /api/download`
  - [ ] `Content-Length`, `Content-Disposition`, `Range`/`206`
  - [ ] refuses directories and special files
- [ ] `GET /api/archive?format=zip|tar.gz`
  - [ ] streamed into the body, ZIP64, no temp files
  - [ ] skips FIFOs/sockets/devices, no symlink following, preserves mtime/mode
  - [ ] `unzip -t` / `tar -tzf` verification in E2E
- [ ] File tree UI
  - [ ] lazy loading, breadcrumbs, sort, hidden toggle, sizes/mtime/mode
  - [ ] keyboard navigation, locked entries, large-directory virtualisation

## 4. Stats

- [ ] Host-wide sampler at 1 Hz, primed so the first CPU sample is valid
- [ ] `GET /api/stats` SSE with subscriber fan-out
- [ ] CPU / RAM / root disk gauges with numeric percentages
- [ ] `3fr / 4px / 1fr` grid, draggable splitter persisted, mobile drawer

## 5. Mutations (`-W` only)

- [ ] CSRF header on every non-GET, default-on, config-free
- [ ] Audit log of actor (from `-H`) + operation + path
- [ ] `POST /api/upload`
  - [ ] multipart, resumable via `Content-Range`, free-space check
  - [ ] upload queue with per-file progress, drag and drop
- [ ] `PATCH`/`DELETE /api/fs`: rename, mkdir, chmod, delete
- [ ] Read-only servers reject every mutation with 403 (E2E)

## 6. Sync and polish

- [ ] Tree follows shell cwd (OSC 7, `/proc/<pid>/cwd` fallback)
- [ ] "Open terminal here", resync control
- [ ] Mobile drawer, accessibility pass on gauges and tree

## Testing and delivery

- [x] Unit tests across CLI, protocol, PTY, auth, routing, state
- [ ] Playwright suite extended to files, stats, upload, CSRF
- [ ] Docker tests (`debian:stable`, non-root): `-u`/`-g`, `-i`, permission-denied paths, `/proc`
- [ ] README and man page
- [ ] Single static binary check (`ldd`)

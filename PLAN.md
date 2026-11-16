# fttyd — ttyd drop-in replacement, with stats sidebar and file manager

A Rust reimplementation of [ttyd](https://github.com/tsl0922/ttyd) (reference clone at `/tmp/ttyd`), extended with a system-stats panel and a navigable file tree with upload/download.

## What "drop-in" means

- Same invocation, same flags, same defaults, same wire protocol, same endpoints, same exit codes.
- Target: `./ttyd` → `./fttyd` with zero other changes to anyone's deployment.
- Verified empirically: ttyd's **own** frontend bundle must work when pointed at fttyd. That is the phase-1 test, and it's a stronger statement than "our frontend works".
- Anything new must be either read-only and default-on, or gated behind `-W`. Nothing new may require a new mandatory flag, or a drop-in swap breaks a running deployment.

## Layout

- `display: grid; grid-template-columns: minmax(0,3fr) 4px minmax(280px,1fr)` — terminal left, sidebar right.
- `minmax(0, …)` on the terminal cell is mandatory; without it the flex/grid item refuses to shrink below xterm's content width and the terminal never reflows.
- Sidebar right, top-to-bottom: CPU gauge, RAM gauge, `/` disk gauge, then the file tree filling remaining height.
- Terminal stays at column 0 when the sidebar collapses — toggling never shifts the terminal's left edge, so shell/TUI output doesn't jump sideways.
- Splitter: draggable, persisted to `localStorage`; sidebar collapses below ~900px into a drawer with the terminal full-bleed.
- Icons: toolbar actions and tree rows use Material Icon Theme SVGs (MIT) rather than Nerd Font glyphs, so nothing depends on the viewer's font; a padlocked folder flags a directory that cannot be opened. Every icon button keeps an `aria-label` — the picture replaces the glyph, not the accessible name.
- Gauges: monospace cells, Unicode blocks (`▁▂▃▄▅▆▇█`) or CSS bar; always print the numeric percent next to it (don't encode state by colour alone); no Nerd Font glyphs, no box-drawing art.

### Terminal resize

Dragging the splitter fires `ResizeObserver` continuously → `FitAddon.fit()` → WS resize → `SIGWINCH` per frame, which makes full-screen TUIs thrash. Debounce ~100 ms and only send `{columns, rows}` when the value actually changed.

## Transports

| Flow | Transport | Why |
|---|---|---|
| PTY bytes | ttyd's exact WebSocket framing | Coupled to the PTY; ordering matters; must stay byte-compatible |
| Files | plain HTTP | Browser-native download progress, `Content-Disposition`, `Range`/`206`, streaming without buffering, `curl`-debuggable |
| Stats | SSE | Monotonic and small; reconnection semantics free; one `EventSource` |

File bytes never travel over the WebSocket: no progress reporting, unbounded memory, and backpressure from a slow tab would stall the PTY.

**Slow-client rule:** one PTY may have several viewer tabs. Use a bounded per-client queue and disconnect a client (close code `1006`) rather than letting it apply backpressure to the shared PTY drain. One frozen tab must not stall `vim` for everyone.

## Protocol parity

Client → server: `0` input · `1` resize + JSON `{columns,rows}` · `2` pause · `3` resume · `{` JSON control.
Server → client: `0` output · `1` window title · `2` preferences.

Handshake details that must match: server pushes title then prefs as the first two binary messages; `GET /token` returns `{"token": "<credential>"}` (ttyd echoes the credential — keep it, it's not a real token); the WS accepts `{"AuthToken": "..."}`; `401` responses carry `Basic realm="ttyd"`; the child process is spawned **per connection** ([protocol.c:155](/tmp/ttyd/src/protocol.c)) with `max_clients` as a global counter; exit code maps to close code `1000`/`1006`.

## New HTTP API

All base-path aware (under `-b`) and namespaced so they can never collide with `/`, `/ws`, `/token`, or a `-I` custom index.

| Endpoint | Purpose |
|---|---|
| `GET /api/fs?path=` | list directory: name, size, mtime, mode, type, symlink target |
| `GET /api/download?path=` | single file, `Content-Length` + `Range`/`206` |
| `GET /api/archive?path=&format=zip\|tar.gz` | folder as a streamed archive |
| `GET /api/stats` | SSE sample stream (CPU, RAM, disk) |
| `PUT /api/upload?path=` | raw body, chunked and resumable via `Content-Range`; `-W` only |
| `POST /api/upload?path=` | `multipart/form-data`, so `curl -F` works; `-W` only |
| `PATCH /api/fs?path=` | rename / mkdir / chmod; `-W` only |
| `DELETE /api/fs?path=` | delete; `-W` only |

Every non-GET requires a custom header matching the value from `/token`.

## Decisions

- **Whole filesystem, no jail.** The terminal beside the tree is already an unconstrained shell, so jailing the tree is security theater that only breaks sysadmin workflows. Add repeatable `--file-root` for people who do want confinement — additive, so it costs parity nothing.
- **`-W` gates everything.** ttyd is read-only by default; `-W` enables terminal input. Extend that one flag to the file API: without it, GET/HEAD only. No new flag, symmetric semantics, and read-only-by-default means the file manager can ship early without any risk to existing deployments.
- **Auth untouched.** `-c` and `-H` behave exactly as ttyd's. No new auth layer: the perimeter is the ALB's SSO, and a second auth system is one more thing to get wrong.
- **CSRF guard instead.** HTTP Basic credentials are attached automatically to cross-site form `POST`s, so a malicious page can blind-POST to the upload endpoint and it authenticates. Require a custom header on all non-GET endpoints: cross-origin JS cannot set one without a preflight, and with no CORS policy the preflight fails. Default-on, config-free, stronger than ttyd's opt-in `-O`.
- **Tree follows the shell.** Each connection has its own child, so each person's tree starts in their own shell's cwd. The session polls `/proc/<pid>/cwd` once a second and pushes it in a new `3` frame; a `/proc` poll beats OSC 7 here because it needs no shell configuration and keeps working after `sudo su` replaces the process. Manual navigation unpins the tree; a toolbar toggle re-pins it. This is the feature ttyd structurally cannot offer.
- **Archives streamed.** zip (default) and tar.gz, written straight into the response body — no temp files, ZIP64 with data descriptors, `Content-Disposition` filename. Skip FIFOs/sockets/devices (reading a FIFO during a tree walk hangs the HTTP handler forever — certain, not theoretical), don't follow symlinks, preserve mtime and unix permissions. Single-file downloads use `Range` so `curl -C -` and download managers resume.
- **Stats sampled host-wide.** One `sysinfo` sampler at 1 Hz for the whole process, fanned out to all sessions; per-connection sampling would waste CPU per viewer. `sysinfo` needs two samples before CPU% is meaningful — the first read is always 0, so prime it at startup.
- **Don't write a terminal emulator.** xterm.js (MIT) + fit + webgl addons; fall back gracefully when `rendererType=webgl` is requested but unavailable.
- **Version string.** `--version` prints the fttyd identity, but keeps ttyd's output *format* so scripts that parse it still work.

## Parity checklist

Everything here must work before "drop-in" is claimed.

| Surface | Requirement |
|---|---|
| Flags | `-p -i -U -c -H -u -g -s -w -a -W -t -T -O -m -o -q -B -I -b -P -6 -S -C -K -A -d -v -h`, same short/long forms, same defaults |
| `-t key=value` | Accept all ttyd keys without choking: `fontSize`, `fontFamily`, `theme`, `titleFixed`, `rendererType`, `closeOnDisconnect`, `disableLeaveAlert`, `disableResizeOverlay`, `enableZmodem`, `enableTrzsz`, `enableSixel`, `scrollback`, `cursorBlink`, `macOptionIsMeta` |
| `-b` base path | Applied to every endpoint including the new ones |
| `-i` / `-U` | UNIX socket bind + socket owner chown |
| `-6` | IPv6 bind |
| `-u` / `-g` | Bind then drop privileges (order matters, or socket perms are wrong) |
| `-s` | Signal to child on exit, default SIGHUP |
| `-o` / `-q` / `-m` | Once / exit-on-last-client / max clients |
| `-S -C -K -A` | rustls; `-A` means client-cert verification — easy to miss, exists for mTLS deployments |
| `-I` | Custom index replaces our UI; the WS protocol must still work for a foreign client |
| `-B` | Open the system browser |
| `-d` | Log level |
| Assets | Embedded, gzipped, cache headers, single static binary |

## Phases

1. **Parity core.** `portable-pty` spawn, ttyd framing, xterm.js, `/ws`, `/token`, `/opt/`, close codes, per-connection child, `-m/-o/-q`/`-p`/`-w`/`-T`/`-s`.
   *Done when:* ttyd's own frontend bundle drives fttyd, including resize, title, and pause/resume.
2. **Parity breadth.** Every remaining row of the checklist: base path, UNIX socket + chown, IPv6, setuid/gid, rustls incl. client certs, embedded assets, `-t` passthrough, `-a` URL args, `-O` origin check, `-B`, `-d`.
   *Done when:* `--help` diffs clean against ttyd's, and every flag has been exercised at least once.
3. **Filesystem read-only.** Tree with lazy loading, breadcrumbs, sort, hidden-file toggle, sizes/mtime/mode, keyboard nav; `/api/fs`, `/api/download`, `/api/archive`.
   *Done when:* browsing `/` and downloading a large directory both behave on a real box.
4. **Stats.** Host-wide sampler, SSE endpoint, three gauges.
5. **Mutations (`-W`).** Upload queue with progress, rename/delete/mkdir/chmod, CSRF header, audit log of `actor + op + path` (actor from `-H`), resumable uploads.
6. **Sync & polish.** Follow-shell-cwd, "open terminal here", OS drag-and-drop upload, mobile drawer, virtualization for huge directories.

Deferred past v1 (accepted in `-t`, inert until implemented): ZMODEM, trzsz, Sixel.

## Status

All six phases are implemented and covered by tests. Two protocol-adjacent additions are
invisible to ttyd's client: the `/api/*` endpoints and the `3` cwd frame.

What is verified where:

| Area | Where |
|---|---|
| CLI surface, protocol framing, PTY, path confinement, archives, uploads, stats, TLS | `cargo test` (96 tests) |
| Terminal behaviour, ttyd-client conformance, sidebar, uploads/downloads, mutations, CSRF, read-only, base path, auth, `-O`/`-m`/`-o`/`-q`/`-a`/`-I`, follow-cwd | `e2e` Playwright suite (46 tests) |
| `-u`/`-g` drop, `-i`/`-U` sockets, permission errors, `/proc` and `/dev` listings, archive of special files | `scripts/docker-tests.sh` |
| Static musl binary (static-pie, HTTP + websocket/PTY + TLS verified inside alpine) | `scripts/build-static.sh` |

`-B` (open a browser) is implemented but not covered by an automated test, because the
test would launch a browser on the machine running it.

## Edge cases

- **`ssm-user` ≠ root.** The file API runs as the server's user while the terminal can `sudo su`, so `/root` and friends are listed as locked and mutations there fail even though the shell can do them. Show the real errno per entry; never fail a whole listing. No root-helper daemon — that's a root-owned file API one parsing bug away from disaster.
- **`/proc`, `/sys`.** One click from `/`. Zero-size files that stream forever, `EIO` on read, `readdir` changing underneath. Per-entry errors only.
- **Disk full** is the top upload failure mode → disk gauge is a feature, plus a free-space check before accepting a large upload.
- Non-UTF-8 filenames, NUL bytes, `lost+found`, permission-denied directories, very deep trees, `..` and symlink escapes in request paths (canonicalize, then reject rather than clamp, so bugs are loud).

## Non-goals

- A local (browser-side) file pane. That's FTP-client legacy; the OS is the local pane — drag-and-drop up, download link down. Optionally progressive-enhanced with the File System Access API on Chromium later.
- Any new authentication system.
- Recursive folder sizes on directory load (unbounded stat storm); compute on demand instead.

## Stack

axum · tokio · tokio-tungstenite · portable-pty · sysinfo · zip · rustls · rust-embed · tower-http · clap.
Frontend: vanilla TypeScript (no framework) + `@xterm/xterm` with the fit, webgl, unicode
and web-links addons, bundled by vite and embedded. The DOM needs here are a terminal,
a splitter, a list and three gauges; a framework would add build weight without buying
anything.

Reference points while building: [src/server.h](/tmp/ttyd/src/server.h) (protocol constants), [src/protocol.c](/tmp/ttyd/src/protocol.c), [src/pty.c](/tmp/ttyd/src/pty.c), [html/src/components/terminal/xterm/index.ts](/tmp/ttyd/html/src/components/terminal/xterm/index.ts) (client options).

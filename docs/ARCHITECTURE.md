# Architecture

How the code is put together, and where to change what. The *why* — transports, security decisions,
edge cases, the record of what was verified how — is in [PLAN.md](../PLAN.md); this file is the map.

## One process, one binary

```
main.rs
  └─ serve()
       ├─ AppState            shared: config, stats, client counter, stop signal
       ├─ bind()              TCP or UNIX socket, before privileges drop
       ├─ drop_privileges()   -u/-g, after the socket exists so its owner is right
       ├─ server::router()    every route, under -b if given
       └─ axum::serve(…).with_graceful_shutdown(state.stopped())
```

The frontend is compiled by `build.rs` (bun → `web/dist`), staged into `$OUT_DIR/web`, and embedded by
`rust-embed`. There is no asset directory at runtime and nothing is read from disk at startup.

## Modules

| Module | Owns |
|---|---|
| `cli.rs` | the hand-written `getopt_long`-compatible parser and the flag table. This is the parity contract; `--help` is generated from it |
| `proto.rs` | frame bytes, control-message parsing, close codes. The client byte that starts a control message *is* `{` |
| `ws.rs` | one terminal session: handshake, `{AuthToken}` check, per-connection child, ping/hangup, cwd polling, shutdown |
| `pty.rs` | the child process via `portable-pty`: setsid, controlling terminal, initial winsize before exec, exit-status mapping |
| `server.rs` | router, auth middleware, index/token/favicon/asset responses, gzip and ETag |
| `auth.rs` | `-c` basic auth, `-H` trusted header, `-O` origin check |
| `api.rs` | `/api/info`, `fs`, `download`, `archive`, `upload`, `stats` handlers; CSRF and `-W` gates |
| `fs.rs` | directory listing, `--file-root` enforcement (canonicalise, then reject), mode strings |
| `archive.rs` | streaming `zip`/`tar.gz` writers fed through a bounded channel |
| `stats.rs` | the single 1 Hz `sysinfo` sampler, the SSE stream, and its shutdown |
| `state.rs` | `AppState`: client accounting for `-o`/`-q`/`-m`, the stop signal everything watches |
| `assets.rs` | the `rust-embed` table, content types, ETags, gzip cache |
| `tls.rs` | rustls config and the TLS accept loop, including `-A` client certificates |

Frontend, in `web/src`: `terminal.ts` (the xterm view and its websocket), `tree.ts` (the file tree,
the largest file), `gauges.ts`, `api.ts` (typed `fetch` helpers), `options.ts` (client and xterm
option defaults, `-t` merging), `modal.ts` (prompt/confirm), `main.ts` (layout and wiring).

## The three request paths

**Terminal.** `GET /ws` upgrades, the client sends `{"AuthToken": …}` with its first `columns`/`rows`,
and the server spawns that connection's child. Output frames flow child → bounded queue → socket; a
slow tab is disconnected rather than allowed to stall the PTY. The session also polls
`/proc/<pid>/cwd` once a second and pushes a `3` frame so the tree can follow `cd`.

**Files.** Plain HTTP, no websocket: `Content-Disposition`, `Range`/`206`, and streaming mean the
browser gives real progress for free and `curl` can debug it.

**Stats.** One `EventSource` per tab on `/api/stats`. The sampler runs once for the whole process and
fans out; per-connection sampling would burn CPU per viewer for identical numbers.

## Adding a route

1. Handler in `api.rs` (or `server.rs` if it is not part of the file API), taking `State<Arc<AppState>>`.
2. Register it in `server::router()` under `cfg.index_path()` so `-b` works.
3. If it mutates anything: call `require_writable` then `require_csrf`, and log through the audit
   helper so `-H` gives an actor.
4. Unit-test the handler with `tower::ServiceExt::oneshot`, the way `server::tests` does.
5. If the browser needs it, add a typed helper in `web/src/api.ts` rather than fetching inline.

## Adding a flag

1. Row in the flag table in `cli.rs` (short, long, takes a value?), plus the field on `Config`.
2. Handle it in the parse loop; keep `--help` output in the same format ttyd uses.
3. If ttyd has the flag, match its default and its meaning exactly — that is what "drop-in" means.
   A divergence is allowed only when ttyd's behaviour is a bug, and it must be added to
   [Divergences](#divergences-from-ttyd).
4. Test it in `cli::tests` (parsing) and, if it changes behaviour, end to end.

## The HTTP API

Everything below is relative to `-b`. GET endpoints work in read-only mode; the rest need `-W` **and**
the CSRF header named by `/api/info` (`x-fttyd-token`), whose value is what `/token` returns.

| Endpoint | Purpose |
|---|---|
| `GET /api/info` | version, writable flag, roots, CSRF header name |
| `GET /api/fs?path=/dir` | one directory level, sorted, with per-entry errors |
| `GET /api/download?path=/file` | file body, `Range` supported |
| `GET /api/archive?path=/dir&format=zip\|tar.gz` | streamed folder archive |
| `GET /api/stats` | server-sent events, one sample per second |
| `PUT /api/upload?path=/dir/file` | raw body, `Content-Range` for resumable chunks |
| `POST /api/upload?path=/dir` | `multipart/form-data`, so `curl -F` works |
| `PATCH /api/fs` | `{"op":"mkdir"\|"rename"\|"chmod", …}` |
| `DELETE /api/fs?path=/target` | remove a file or directory |

```sh
TOKEN=$(curl -s -u user:pass http://127.0.0.1:7681/token | jq -r .token)
curl -u user:pass -H "x-fttyd-token: $TOKEN" -T file.bin \
     'http://127.0.0.1:7681/api/upload?path=/tmp/file.bin'
```

The header is not ceremony: browsers attach HTTP Basic credentials to cross-site form posts, so
without it a malicious page could upload to your server. A custom header cannot be set cross-origin
without a preflight, and no CORS policy is granted.

## Divergences from ttyd

Each of these is deliberate, and each is a case where ttyd's behaviour costs the user something:

- **Option parsing stops at the command** (`--` implied). glibc's `getopt_long` permutes arguments, so
  `ttyd bash -l` silently drops `-l` and `ttyd bash -W` turns on writable mode.
- **Unknown options are an error** instead of being logged and ignored.
- **Numeric options reject trailing garbage** (`-p 8080x`) instead of truncating.
- **`-t` takes one `key=value` per occurrence**, as its own `--help` documents.
- **Two protocol additions**, both invisible to ttyd's client: the `3` cwd frame and `/api/*`.

## Build system notes

- `build.rs` writes only to `$OUT_DIR`. `cargo publish` refuses to verify a package whose build script
  modified it, which is how that rule got learned.
- It rebuilds the bundle when any file under `web/src`, `web/public`, the vite/ts configs or
  `bun.lock` is newer than `web/dist/index.html`. A new input directory must be added to both
  `rerun-if-changed` and `needs_build`.
- A packaged crate has no `node_modules`, so `build.rs` reuses the shipped bundle: `cargo install fttyd`
  needs no Node toolchain.

## Testing

| Suite | Covers |
|---|---|
| `cargo test` | CLI parsing, protocol framing, PTY, path confinement, archives, uploads, stats, TLS, routing, shutdown |
| `e2e/` (Playwright) | the real binary in Chromium: terminal I/O, ttyd-client conformance, sidebar, drag-and-drop upload, downloads, mutations, CSRF, read-only, base path, auth, follow-cwd, `-B` |
| `scripts/docker-tests.sh` | privilege dropping, socket ownership, permission errors, `/proc` and `/dev` listing, archives of special files |

Run the smallest command that covers your change; `AGENTS.md` has the exact invocations and the
mistakes worth avoiding.

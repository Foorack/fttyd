# fttyd

A web terminal in Rust that is a **drop-in replacement for [ttyd](https://github.com/tsl0922/ttyd)** — same flags, same wire protocol, same endpoints — with a sidebar that shows live CPU/memory/disk gauges and a navigable file tree with upload and download.

Replace `ttyd` with `fttyd` in an existing invocation and nothing else changes. The terminal part is honest to ttyd down to the byte: ttyd's own built client runs against it unmodified, which is what the conformance test asserts.

## Build

```sh
cargo build --release
```

One binary. The frontend is built with [bun](https://bun.sh) (or any `npm`) by `build.rs` on the first build and embedded, so the result has no runtime asset directory and no network dependencies.

```sh
cargo test                       # unit tests
(cd e2e && bun install && bunx playwright install chromium && bunx playwright test)
./scripts/docker-tests.sh        # privilege, socket and special-file checks (needs docker)
```

## Usage

```
fttyd [options] <command> [<arguments...>]
```

Every ttyd option is accepted with the same meaning and default; `fttyd --help` differs from `ttyd --help` only in the program name, the version and one added option:

```
fttyd -W -p 8080 bash
fttyd -W -c user:pass -b /mounted/here bash
fttyd -W -i /var/run/fttyd.sock -U nobody:nogroup bash
fttyd -W -S -C cert.pem -K key.pem bash          # TLS, and -A ca.pem requires a client cert
fttyd -W --file-root /srv bash                  # confine the file API (fttyd addition)
```

Read-only is the default, exactly like ttyd: without `-W`, clients cannot type into the terminal **and** the file API is read-only.

### Sidebar

- **Gauges** — CPU, memory and the disk that holds the current directory, drawn in monospace block characters with the numeric value beside every bar.
- **File tree** — lazy-loading, keyboard navigable, sorted directories first, hidden-file toggle, sizes and mtimes, symlinks marked and never followed.
- **Downloads** — single files with `Range` support, folders streamed as `zip` or `tar.gz` (ZIP64, mtime and permissions preserved, nothing written to disk). A folder download of a tree containing unreadable files or special files still succeeds; what was skipped is listed in `fttyd-archive-report.txt` inside the archive.
- **Uploads** — chunked and resumable, with a free-space check before the first byte and an atomic rename when the last chunk lands. Drag files (or folders) from your desktop onto the tree.
- **Follow the shell** — the tree tracks the shell's working directory through `/proc`; navigating by hand pins it, and the ◎ button jumps back.
- **Open terminal here** — types a quoted `cd` into the attached terminal.

The splitter is draggable and remembered; below 900px the sidebar becomes a drawer so the terminal keeps the full width.

## API

All endpoints are relative to `-b`. GET endpoints are available in read-only mode.

| Endpoint | Purpose |
|---|---|
| `GET /api/info` | version, writable flag, root(s), CSRF header name |
| `GET /api/fs?path=/dir` | one directory level, sorted |
| `GET /api/download?path=/file` | file body, `Range` supported |
| `GET /api/archive?path=/dir&format=zip\|tar.gz` | streamed folder archive |
| `GET /api/stats` | server-sent events, one sample per second |
| `PUT /api/upload?path=/dir/file` | raw body, `Content-Range` for resumable chunks |
| `POST /api/upload?path=/dir` | `multipart/form-data` (so `curl -F` works) |
| `PATCH /api/fs` | `{"op":"mkdir"\|"rename"\|"chmod", ...}` |
| `DELETE /api/fs?path=/target` | remove a file or directory |

Uploads and mutations require `-W` **and** the CSRF header named by `/api/info` (`x-fttyd-token`), whose value is what `/token` returns:

```sh
TOKEN=$(curl -s -u user:pass http://127.0.0.1:7681/token | jq -r .token)
curl -u user:pass -H "x-fttyd-token: $TOKEN" -T file.bin \
     'http://127.0.0.1:7681/api/upload?path=/tmp/file.bin'
```

The header is required because browsers attach HTTP Basic credentials to cross-site form posts: without it, a malicious page could upload a file to your server. A custom header cannot be set cross-origin without a preflight, and no CORS policy is granted.

## Security model

- **The perimeter is yours.** `-H` (trust an authenticating reverse proxy) and `-c` (HTTP Basic) behave exactly as in ttyd. Behind SSO or a VPN, `-H` also gives every file operation an audit trail: mutations log `file op: <op> <path> actor=<header value>`.
- **Read-only by default.** `-W` is the single switch for terminal input and file mutations.
- **The tree has the same reach as the shell.** By default it can see the whole filesystem, because the terminal next to it can too. `--file-root` (repeatable) confines it, resolving symlinks so a link cannot escape.
- **Client certificates** are enforced when `-A` is given. rustls does not implement TLS 1.0/1.1 at all, which is at least as strict as ttyd.
- The file API runs with the server's privileges. It cannot `sudo` like an interactive shell can, so root-owned paths are listed as locked with the real error rather than failing the listing.

## Compatibility notes

Behaviour matches ttyd except where ttyd silently does the wrong thing. Each of these is a deliberate, documented divergence:

- **Option parsing stops at the command.** glibc's `getopt_long` permutes arguments, so `ttyd bash -l` silently drops `-l` and `ttyd bash -W` turns on writable mode. fttyd treats `--` as implied, so command flags reach the command.
- **Unknown options are an error** instead of being logged and ignored.
- **Numeric options reject trailing garbage** (`-p 8080x`) instead of truncating.
- **`-t` takes one `key=value` per occurrence**, as `--help` documents.
- **Two protocol additions**, both invisible to ttyd's client: a `3` frame carrying the child's working directory, and the `/api/*` endpoints.

## License

MIT. ttyd is MIT as well; the wire protocol and option surface are reimplemented from its behaviour, and its shipped client is used only as a test fixture.

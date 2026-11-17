# fttyd

A web terminal in Rust that is a **drop-in replacement for [ttyd](https://github.com/tsl0922/ttyd)** — same flags, same wire protocol, same endpoints — with a sidebar that shows live CPU/memory/disk gauges and a navigable file tree with upload and download.

Replace `ttyd` with `fttyd` in an existing invocation and nothing else changes. The terminal part is honest to ttyd down to the byte: ttyd's own built client runs against it unmodified, which is what the conformance test asserts.

## Build

```sh
cargo build --release          # one binary, glibc
./scripts/build-static.sh      # one binary, no libc at all (needs docker)
```

The frontend is built with [bun](https://bun.sh) (or any `npm`) by `build.rs` on the first build and embedded, so the result has no runtime asset directory and no network dependencies. `build.rs` compiles it into `web/dist`, copies that into `OUT_DIR`, and `rust-embed` reads it from there — so the build never writes outside `OUT_DIR`, which is what `cargo publish` requires.

**From crates.io.** The published crate ships the built bundle, so `cargo install fttyd` compiles it without needing bun or a network.

**Releasing.** Pushing a `v*` tag runs [`.github/workflows/release.yml`](.github/workflows/release.yml), which builds static `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` binaries with [`cross`](https://github.com/cross-rs/cross), attaches them (plus `SHA256SUMS`) to a GitHub release, and publishes the crate. crates.io authentication is trusted publishing: the job asks GitHub for an OIDC token, exchanges it for a 30-minute crates.io token, and no API token is stored in the repository. The workflow checks that the tag matches `Cargo.toml` before spending a version number, and `workflow_dispatch` rehearses the whole thing with `cargo publish --dry-run`.

Trusted publishing only works for a crate that already exists, so the first version was published with a token; the crate's Settings → Trusted Publishing must then name this repository, the workflow file `release.yml` and the `release` environment. Locally:

```sh
(cd web && bun run build) && cargo publish --allow-dirty
```

**Binaries** are also built by `./scripts/build-static.sh` for the host architecture, which needs only docker.

**Static builds.** The only non-Rust dependency is a C compiler that targets musl, because `ring` (rustls's crypto backend) assembles C. On Debian/Ubuntu:

```sh
sudo apt install musl-tools
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

`musl-tools` installs `x86_64-linux-musl-gcc`, which cc-rs finds by name, so no `CC` override is needed. Distributions that only ship the generic wrapper need `CC_x86_64_unknown_linux_musl=musl-gcc`.

`./scripts/build-static.sh [output]` does all of that, and falls back to building inside `rust:1-alpine` when the host has no musl toolchain (needs only docker). Either way the result is `static-pie linked`, is checked to actually be static, and runs on alpine or `scratch` images.

```sh
cargo test                       # unit tests
(cd e2e && bun install && bunx playwright install chromium && bunx playwright test)
./scripts/docker-tests.sh        # privilege, socket and special-file checks (needs docker)
```

## CI

`.github/workflows/ci.yml` runs on every push to every branch and on every pull request:

| Job | What it enforces |
|---|---|
| `author check` | no commit in the pushed range credits an AI model |
| `cargo fmt` | `cargo fmt --all -- --check` |
| `frontend typecheck` | `bun run typecheck` plus a production build of the bundle |
| `cargo clippy` | `--all-targets` with `-D warnings` |
| `cargo test` | the Rust unit tests |
| `playwright` | builds the debug binary, installs chromium, runs the `e2e` suite, keeps `e2e/test-results` on failure |
| `container checks` | `scripts/docker-tests.sh`: privilege dropping, socket ownership, permission errors, special files |

The browser and docker suites run there too, so a locally passing run and CI agree. Locally
they need a browser download and docker respectively; run them with the commands under
[Build](#build).

Local equivalents:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
(cd web && bun run typecheck)
./scripts/test-check-authors.sh     # checks the author checker itself
```

### The author check

`scripts/check-authors.sh [range]` fails when any commit in the range

- has an author or committer whose name or email names an AI model or vendor,
- carries an attribution trailer (`Co-authored-by`, `Assisted-by`, `Generated-by`, ...)
  naming one, or
- contains a "generated with/by <model>" notice.

Tokens covered include Copilot, Claude, ChatGPT/OpenAI, Gemini/Bard, Cursor, Codeium,
Windsurf, Codex, Devin, Aider, Tabnine, Amazon Q, Mistral, Llama, DeepSeek, Grok and
others. Mentioning a model in prose is allowed, so a commit *about* removing such
attribution still passes.

The same rule runs before a commit exists, if you opt in once per clone:

```sh
git config core.hooksPath .githooks
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
- **File tree** — lazy-loading, keyboard navigable, sorted directories first, hidden-file toggle, sizes and mtimes, symlinks marked and never descended into. Entries whose names are not valid UTF-8 cannot be addressed over JSON, so the status line says how many were omitted rather than hiding them silently.
- **Downloads** — single files with `Range` support (a symlink is followed to its target), folders streamed as `zip` or `tar.gz` (ZIP64, mtime and permissions preserved, nothing written to disk). A folder download of a tree containing unreadable files or special files still succeeds; what was skipped is listed in `fttyd-archive-report.txt` inside the archive. Archives never follow symlinks, so a link is stored as a link.
- **Uploads** — chunked and resumable, with a free-space check before the first byte and an atomic rename when the last chunk lands. Drag files (or folders) from your desktop onto the tree.
- **Follow the shell** — the tree tracks the shell's working directory through `/proc`; navigating by hand pins it, and the ◎ button jumps back.
- **Icons** — the toolbar and the tree use the [Material Icon Theme](https://github.com/material-extensions/vscode-material-icon-theme) set (MIT): a padlock folder marks a directory that cannot be opened, and the tab icon is original artwork rather than a vendor logo: a dark slate tile with an amber-to-orange `>_` prompt. See [web/src/icons/README.md](web/src/icons/README.md).
- **Walking up** — the tree is not anchored to the directory `fttyd` was started in: each ↑ asks the server for the parent and rebases the view one level up, keeping expansion, all the way to `/`. A confined `--file-root` stops it at the boundary with an explanation instead of silently going nowhere.
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
- **Ctrl+C stops the server.** SIGINT/SIGTERM close every open session (websockets get a `1001` close frame, the stats event source ends) and then exit, instead of waiting for browsers to go away first.
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

MIT. ttyd is MIT as well; the wire protocol and option surface are reimplemented from its behaviour, and its shipped client is used only as a test fixture. The sidebar icons come from Material Icon Theme (MIT), whose licence sits in [web/src/icons/](web/src/icons/).

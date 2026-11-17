# AGENTS.md

Instructions for any agent working in this repository. Written from what actually went wrong
and what actually took time, not from what reads well.

## What this is

`fttyd` is a Rust reimplementation of [ttyd](https://github.com/tsl0922/ttyd): a web terminal
that is a **drop-in replacement** (same flags, same wire protocol, same endpoints, same exit
codes) and adds a stats sidebar with a file manager. One binary, frontend embedded.

The parity rule outranks everything else: a change that makes fttyd better but breaks ttyd's
own client against it is a regression, not a feature. `e2e/tests/conformance.spec.ts` drives
ttyd's shipped bundle against this server and is the gate for that.

`PLAN.md` holds the design rationale and the verified-where table. Read it before changing
protocol, transport or API decisions; those decisions have reasons recorded there.

## Hard rules

- **Do not push.** Commits are fine. Pushing, tagging, releasing and force-pushing happen only
  when the user says so in that message.
- **Do not delete, rewrite or "consolidate" a document the user did not ask you to touch.**
  Deleting `TODO.md` because a form answer could be read that way is exactly the failure this
  line exists for. If a change removes content the user wrote, ask first, in its own question,
  not as one option in a multiple-choice list.
- **No AI attribution, ever.** No `Co-authored-by`, no `Generated with`, no model named in a
  trailer or identity. CI enforces it (`scripts/check-authors.sh`), a local hook enforces it
  earlier (`core.hooksPath=.githooks`), and the whole history was rewritten once to purge it.
  Never cite the assistant when describing work in docs or commit messages.
- **Do not run `git add -A` or `git add .`.** Twice a comparison screenshot got committed that
  way. Stage the files you actually changed, by name, and check `git status --short` first.
- **Do not kill processes by name.** Kill by PID, and never kill a server the user is running
  (e.g. their `--port 8080` session) while cleaning up after yourself.
- **No `.tmp*`, `*.png` or scratch files in the repo root.** Put scratch artifacts in `/tmp`,
  or in the session workspace, and delete them when done.

## Commands

```sh
cargo build                     # debug binary; build.rs builds the frontend
cargo test                      # 98 unit/integration tests
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings

(cd web && bun run typecheck)   # tsc --noEmit
(cd web && bun run build)       # vite build into web/dist

(cd e2e && bunx playwright test)                       # 50 tests, needs `cargo build` first
(cd e2e && bunx playwright test tests/sidebar.spec.ts)  # one file
(cd e2e && bunx playwright test -g "climb")             # one test by name

./scripts/docker-tests.sh       # privileges, sockets, special files (needs docker)
bash scripts/test-check-authors.sh   # 21 self-checks of the attribution ban
actionlint .github/workflows/*.yml   # not vendored; grab the release tarball if missing
```

`--locked` fails after bumping the version in `Cargo.toml` (the lockfile records the package
version). Refresh with `cargo build --offline`, then `--locked` works again.

## Layout

| Path | What lives there |
|---|---|
| `src/cli.rs` | hand-written `getopt_long`-compatible parser; the flag table is the parity contract |
| `src/proto.rs` | frame bytes and close codes; the client byte for a control message **is** `{` |
| `src/ws.rs` | terminal session: handshake, ping/hangup, cwd polling, shutdown |
| `src/pty.rs` | `portable-pty` child; `PtyControl` holds `Mutex<Box<dyn MasterPty + Send>>` because `MasterPty` is `Send` but not `Sync` |
| `src/server.rs` | router, auth middleware, asset and favicon responses |
| `src/api.rs` | `/api/*`: list, download, archive, upload, mutate, delete |
| `src/fs.rs` | listing, `--file-root` enforcement, mode strings |
| `src/archive.rs` | streaming zip/tar.gz (never buffer, never follow symlinks) |
| `src/stats.rs` | one 1 Hz `sysinfo` sampler fanned out over SSE |
| `src/state.rs` | `AppState`: client counting, stop signal |
| `src/assets.rs` | `rust-embed` over `$OUT_DIR/web` |
| `web/src/*.ts` | no framework: terminal, tree, gauges, modal, api, options |
| `e2e/` | Playwright suite that drives the real binary |

## Build system

`build.rs` runs `bun run build` into `web/dist`, then copies the result into `$OUT_DIR/web`,
which is what `rust-embed` reads. It must not write anywhere else — `cargo publish` rejects a
build script that modifies the package being verified, which is how that rule was discovered.

Consequences worth knowing:

- `bun` is required for a normal build, but **not** for `cargo install` from a published crate:
  the bundle ships in the tarball and `build.rs` reuses it when there is no `web/node_modules`.
- `build.rs` watches `web/src`, `web/public`, the vite/ts configs and `bun.lock`. If you add a
  new input directory, add it to both the `rerun-if-changed` list and `needs_build`.
- `web/dist` is generated and gitignored; publishing passes `--allow-dirty` because of it.

## Testing

- Verify the claim, not the code: a new test is only useful if it **fails before the fix**.
  Temporarily revert the fix, watch the test go red, restore it, watch it go green. This caught
  two bugs that "looked" fixed.
- Assert on something that only becomes true after the change. `toContainText` on text that was
  already there passes immediately and tests nothing; use `toHaveText` on the specific element,
  or `expect.poll`.
- Prefer E2E for anything the browser can see and unit tests for anything the parser can see.
  Don't write a test that restates the implementation.
- Keep it focused. The owner has objected to over-testing; a handful of tests covering the
  behaviour beats a suite that mirrors the code.

### Playwright gotchas

- `locator.dblclick()` on a chained locator can compute a wrong click point; use
  `page.locator(...).dblclick()`.
- A screenshot of the terminal needs the DOM renderer, because headless WebGL paints nothing:
  start the server with `-t 'rendererType="dom"'` (note the quoted value — the parser wants a
  JSON-ish value, and bare `rendererType=dom` is not the same thing).
- A context menu closed on `pointerdown` will vanish before its item is clicked; guard the
  handler or press `Escape` and assert the menu is gone before clicking underneath it.
- Tests that `chmod 000` a directory must restore the mode before cleanup, or the recursive
  delete fails with `EACCES`.
- Fixtures come from `makeFixture(name)`; `startServer()` picks an ephemeral port and accepts an
  `env` override, which is how `-B` is tested against a stub `xdg-open`.

## Parity notes

- Byte-level compatibility is non-negotiable: server sends `1<title>` then `2<prefs>`, client
  control messages start with `{`, exit codes map to close codes `1000`/`1006`.
- New endpoints are namespaced under `/api/` and base-path aware, so they cannot collide with
  `/`, `/ws`, `/token` or a `-I` custom index.
- Anything new is read-only by default or gated behind `-W`. No new mandatory flag.
- Divergences from ttyd are deliberate and documented in the README's compatibility section.
  Adding one without documenting it is the same as not having decided it.

## Versioning and release

- `0.0.2` is published on crates.io and crates.io versions are immutable: the next release is
  `0.0.3`. A version bump touches `Cargo.toml`, `web/package.json` and `Cargo.lock`.
- `.github/workflows/release.yml` runs on a `v*` tag: it refuses to continue if the tag and
  `Cargo.toml` disagree, builds static `x86_64`/`aarch64-unknown-linux-musl` binaries with
  `cross`, attaches them plus `SHA256SUMS` to a GitHub release, and publishes via OIDC trusted
  publishing. No API token exists anywhere; the workflow file name, the repository and the
  `release` environment must match what is configured on crates.io.
- `workflow_dispatch` rehearses the whole pipeline with `--dry-run` without spending a version.

## Style

- Match the surrounding code. Comments explain *why*, only where a reader would otherwise
  wonder; no narration of what the next line does.
- Error messages are for the person reading a log at 2am: say what failed and what to do.
- Prefer extending an existing module to adding one. This codebase is deliberately flat.

## When to ask

Ask when a decision is the owner's: file deletions, public-facing naming, palette/icon
choices, anything irreversible (history rewrites, tags, deletes, force pushes), and anything
that changes what a user sees without being requested.

Do not ask about implementation trivia, and do not present a destructive action as one option
among several — give it its own yes/no question and spell out exactly what it removes.

# AGENTS.md

`fttyd` is a Rust rewrite of ttyd: a web terminal in one binary, with the frontend embedded and a stats sidebar that carries a file manager.

## Simplified Technical English

These rules apply to all prose you write: docs, commit messages, pull request descriptions, reports, and replies.

Follow ASD-STE100 Simplified Technical English for technical text:

- Use approved words only. Each word has one meaning.
- Use one word for one idea. Do not use two words for the same thing.
- Write short sentences. Use 20 words or less for instructions.
- Use active voice. Write "Turn the switch", not "The switch must be turned".
- Write short paragraphs. Keep one topic in each paragraph.

## Replies

- Keep replies short. Give only the most important information. Three or four lines for a finished task, also for a review or an audit. Give the verdict first. Do not recap the plan, list every file, or explain the same thing twice. Leave out details that the user does not need to act. Give details only when the user asks. A wall of text is a failed reply.
- When the user must decide, use the AskUserQuestion tool. Give two to four options, with your recommendation first. Do not put the choice in a long reply.

## Project

Parity outranks every other goal. A change that makes fttyd better but breaks ttyd's own client against it is a regression.

`e2e/tests/conformance.spec.ts` drives ttyd's own bundle against this server. That test is the parity gate. It skips when `e2e/.cache/ttyd-client.html` is missing. `scripts/extract-ttyd-frontend.py` creates it.

`PLAN.md` holds the design rationale and the verified-where table. Read it before you change a protocol, transport or API decision. Every one of those has a reason recorded there.

- `src/cli.rs` - hand-written `getopt_long` parser. The flag table is the parity contract.
- `src/proto.rs` - frame bytes and close codes.
- `src/ws.rs` - terminal session: handshake, ping and hangup, cwd polling, shutdown.
- `src/pty.rs` - `portable-pty` child. `PtyControl` holds `Mutex<Box<dyn MasterPty + Send>>`, because `MasterPty` is `Send` but not `Sync`.
- `src/server.rs` - router, asset and favicon responses, base path.
- `src/auth.rs` - `-c` credential and `-H` proxy header, matching ttyd.
- `src/api.rs` - `/api/*`: list, download, archive, upload, mutate, delete, plus the CSRF check.
- `src/fs.rs` - listing, `--file-root` enforcement, mode strings.
- `src/archive.rs` - streaming zip and tar.gz.
- `src/stats.rs` - one 1 Hz `sysinfo` sampler, fanned out over SSE.
- `src/state.rs` - `AppState`: client counting, stop signal.
- `src/tls.rs` - `--ssl` termination, including `--ssl-ca` client certificates.
- `src/assets.rs` - `rust-embed` over `$OUT_DIR/web`.
- `web/src/*.ts` - no framework: terminal, tree, gauges, modal, api, options.
- `e2e/` - Playwright suite that drives the real binary. `docs/` holds the architecture note and the screenshots.

## Commands

| What                | How                                                         |
| ------------------- | ----------------------------------------------------------- |
| Build               | `cargo build` (builds the frontend too)                     |
| Test Rust           | `cargo test` (98 tests)                                     |
| Format              | `cargo fmt --all -- --check`                                |
| Lint Rust           | `cargo clippy --all-targets -- -D warnings`                 |
| Typecheck frontend  | `(cd web && bun run typecheck)`                             |
| Build frontend      | `(cd web && bun run build)`                                 |
| Test browser        | `(cd e2e && bunx playwright test)` (50 tests)               |
| One browser file    | `(cd e2e && bunx playwright test tests/sidebar.spec.ts)`    |
| One browser test    | `(cd e2e && bunx playwright test -g "climb")`               |
| Test privileges     | `./scripts/docker-tests.sh` (needs docker)                  |
| Check attribution   | `bash scripts/check-authors.sh`                             |
| Test the checker    | `bash scripts/test-check-authors.sh` (21 self-checks)       |
| Lint workflows      | `actionlint .github/workflows/*.yml` (not vendored)         |

The browser suite needs `cargo build` first, because it runs `target/debug/fttyd`.

## Rules

**⚠️ Never use a Unicode dash.** Use the ASCII hyphen `-` only. Check every file you write or edit with `grep -nP '[\x{2010}-\x{2015}\x{2212}]' <file>`, and fix each hit before you finish.

**⚠️ Commit, never push.** A commit is fine and expected. A push, a tag, a release and a force push happen only when the user asks for one in that same message. Do not watch CI either: report and stop.

**⚠️ Never attribute work to an AI.** No `Co-authored-by`, no "Generated with", no model named in a trailer, an identity or a doc line. `scripts/check-authors.sh` fails the build on one. A local hook in `.githooks`, enabled by `core.hooksPath`, fails the commit earlier. The whole history was rewritten once to purge what got in. Never cite the assistant in a commit message or a document.

**⚠️ Never run `git add -A` or `git add .`.** A comparison screenshot was committed that way twice. Run `git status --short`, then stage the files you changed, by name.

**⚠️ Never delete, rename or merge a document the user did not ask about.** A deleted `TODO.md` is the failure this line exists for. Twice this has cost trust. If a change would remove content the user wrote, ask first.

**⚠️ A form answer is not consent for a destructive action.** A deletion needs its own yes/no question, and the question must name exactly what it removes. Never put a deletion as one option among several.

**⚠️ No confidential data in an image or a doc.** Screenshots are published on GitHub. No real username, home path, hostname, uptime, process table, IP address or credential. Take a shot against a synthetic fixture, never the machine you sit at. Recipe and pre-commit checklist: `docs/img/README.md`.

**⚠️ Do not promise what the code does not do.** Binaries exist for `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` only. ZMODEM, trzsz and Sixel are accepted in `-t` and do nothing. There is no accessibility audit. `--file-root` confines the file API; it is not a jail. If a doc claims otherwise, fix the doc.

**⚠️ Never kill a process by name.** Kill the PID. Never kill a server the user is running, for example their `--port 8080` session, while you clean up after yourself.

**⚠️ No scratch files in the repo root.** No `.tmp*`, no `*.png`, no notes. Use `/tmp` or the session workspace, and delete them when the work is done.

### Working rules

- **Commit each finished step.** Do it when the checks pass, without being asked. One small commit per step, and keep unrelated changes apart.
- **Ask when the decision is the owner's.** File deletions, public naming, palette and icon choices, anything irreversible (a tag, a history rewrite, a delete), and anything a user sees that nobody asked for. Do not ask about implementation trivia.

## Build

`build.rs` runs `bun run build` into `web/dist`, then copies the result into `$OUT_DIR/web`, which `rust-embed` reads. It writes nowhere else. `cargo publish` refuses to verify a package whose build script modified it, and that is how the rule was found.

- `bun` is needed for a normal build, not for `cargo install` from a published crate. The bundle ships in the tarball, and `build.rs` reuses it when `web/node_modules` is absent.
- `build.rs` watches `web/src`, `web/public`, `web/index.html`, `web/package.json`, `web/vite.config.ts`, `web/tsconfig.json` and `web/bun.lock`. A new input directory goes in that list and in `needs_build`.
- `web/dist` is generated and gitignored. Publishing passes `--allow-dirty` because of it.

## Testing

- **Verify the claim, not the code.** A test is only useful if it fails before the fix. Revert the fix, watch the test go red, restore it, watch it go green. Two bugs that "looked" fixed survived until this was done.
- **Assert on something that only becomes true after the change.** `toContainText` on text that was already there passes at once and tests nothing. Use `toHaveText` on the specific element, or `expect.poll`.
- **E2E for what a browser sees, unit tests for what the parser sees.** Do not write a test that restates the implementation.
- **Keep it focused.** The owner has objected to over-testing. A handful of tests for the behaviour beats a suite that mirrors the code.

### Playwright gotchas

- `locator.dblclick()` on a chained locator can compute a wrong click point. Use `page.locator(...).dblclick()`.
- A terminal screenshot needs the DOM renderer, because headless WebGL paints nothing. Start the server with `-t 'rendererType="dom"'`. The quoted value matters. `rendererType=dom` is a different thing.
- A context menu closed on `pointerdown` vanishes before its item can be clicked. Guard the handler, or press Escape and assert the menu is gone before you click underneath it.
- A test that runs `chmod 000` must restore the mode before cleanup, or the recursive delete fails with `EACCES`.
- Fixtures come from `makeFixture(name)`. `startServer()` picks an ephemeral port and takes an `env` override. That is how `-B` is tested against a stub `xdg-open`.

## Decisions - do not open these again

- **The wire protocol stays byte-identical to ttyd.** The server sends `1<title>` then `2<prefs>`. A client control message starts with `{`. Exit codes map to close codes `1000` and `1006`.
- **New endpoints live under `/api/`.** They are base-path aware, so they cannot collide with `/`, `/ws`, `/token` or an `-I` custom index.
- **Read-only is the default.** `-W` gates every mutation. Never add a mandatory flag.
- **The whole filesystem is reachable** unless `--file-root` is set. Then the file API refuses paths outside those directories. That is confinement, not a jail.
- **Authentication stays ttyd's.** `-c` for a basic credential, `-H` for a header set by an authenticating proxy, plus one CSRF header, `x-fttyd-token`, on each mutating request. Do not add a session layer.
- **The frontend stays framework-free.** Plain TypeScript in `web/src/`. No React, no router, no component library.
- **The palette is settled.** `web/src/options.ts` carries the owner's terminal profile: pure primaries, bold-is-bright, on a near-black slate. Do not retune it unless the user asks.
- **A divergence from ttyd is deliberate and documented** in the README compatibility section. An undocumented divergence counts as undecided.
- **Archives stream.** `src/archive.rs` never buffers a whole file and never follows a symlink.
- **Stats sample once.** One sampler at 1 Hz fans out over SSE. Do not sample per client.

## AI Boundaries

| Scope     | Allowed                                                              | Forbidden                                                            |
| --------- | -------------------------------------------------------------------- | -------------------------------------------------------------------- |
| Source    | `src/`, `web/src/`, `build.rs`, `e2e/`                               | rewriting history, `git rebase`, `git reset --hard`                  |
| Docs      | `AGENTS.md`, `docs/`, and a doc the change makes wrong               | deleting, renaming or merging a doc nobody asked about               |
| Git       | `git status`, `git diff`, `git log`, `git add <path>`, `git commit`  | `git add -A`, `git add .`, `git push`, `git tag`, `git push --force` |
| Checks    | `cargo build/test/fmt/clippy`, `bun run typecheck/build`, playwright | editing a test or a lint to make it pass, skipping a failing check   |
| Images    | a screenshot of a synthetic fixture                                  | any real account, host, path, uptime or process table                |
| Processes | kill a PID you started                                               | `pkill`, `killall`, killing the user's server                        |

## Release

Read the current version from `Cargo.toml`; do not copy a number out of a document, because this
file and every other one would then need editing on each release. crates.io versions are immutable:
raise the patch for a fix, the minor for a change a user can see.

- A bump touches `Cargo.toml`, `web/package.json` and `Cargo.lock`. `--locked` fails until the lockfile is refreshed with `cargo build --offline`. Do not write the number into docs; the README's download URL is the only place that needs it, and that is deliberate so the asset link works.
- `.github/workflows/release.yml` runs on a `v*` tag. It stops when the tag and `Cargo.toml` disagree. It builds the two static musl binaries with `cross`, attaches them and `SHA256SUMS` to a GitHub release, and publishes with OIDC trusted publishing. No API token exists anywhere.
- The workflow name, the repository and the `release` environment must match the crates.io settings.
- `workflow_dispatch` rehearses the pipeline with `--dry-run` and spends no version.
- **Tagging and pushing a tag is a release.** Never do it unless the user asks in that message.

## Style

- Comments explain why. Do not narrate what the next line does.
- An error message is for the person reading a log at 2am: say what failed and what to do.
- Prefer extending an existing module to adding one. This codebase is flat on purpose.
- Match the surrounding code, including its error type and its test style.

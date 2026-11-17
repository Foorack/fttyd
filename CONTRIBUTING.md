# Contributing

Bug reports and small pull requests are welcome. For anything larger, open an issue first so we agree
on the shape of it before you spend an evening on it.

## Build and test

```sh
cargo build --release                # needs bun (or npm) - build.rs compiles the frontend
cargo test                           # Rust tests
(cd e2e && bun install && bunx playwright test)   # drives the real binary in Chromium
./scripts/docker-tests.sh            # privileges, sockets, special files (needs docker)
```

`cargo fmt --all -- --check` and `cargo clippy --all-targets -- -D warnings` must be clean; CI runs
both, along with the three suites above.

## Rules that CI enforces

- **No AI attribution.** No `Co-authored-by`, `Assisted-by` or "generated with" line naming a model,
  and no author or committer that names one. `scripts/check-authors.sh` checks every commit in the
  pushed range. Enable the local guard once per clone and you will find out before pushing:

  ```sh
  git config core.hooksPath .githooks
  ```

- **Formatting, lints and tests** as above.

- Please do not push to `main`. Work on a branch and open a pull request; the same checks run there.

## What a good change looks like

- One idea per pull request, with the reasoning in the description.
- A test that fails before your change and passes after it, in the suite that owns the behaviour:
  `cargo test` for parsing and server logic, `e2e/` for anything a browser can see.
- Behaviour matching ttyd unless there is a reason not to. If you deliberately diverge, say why in the
  description and add it to `docs/ARCHITECTURE.md#divergences-from-ttyd`.
- New flags need a row in the `cli.rs` flag table, `--help` text in ttyd's format, and a parsing test.

[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) maps the modules and shows how to add a route or a flag.
`AGENTS.md` collects the sharp edges, including the build and test commands.

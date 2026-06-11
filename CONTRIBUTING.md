# Contributing to Ekurl

Thanks for your interest in improving Ekurl! This guide covers local development, the checks your change must pass, and the pull-request flow.

## Development environment

Docker is the recommended way to build and serve the app locally — it matches the production build and keeps your host clean.

```bash
# Build and serve on http://localhost:8080
docker compose up
```

If you prefer a native toolchain, install the latest stable Rust (via [rustup](https://rustup.rs/)) and run:

```bash
cargo run        # starts the server on http://0.0.0.0:8080 (or $PORT)
```

The SQLite database is created at `data/ekurl.db` and the `data/` directory is git-ignored.

## Before you open a pull request

CI runs the checks below on every push and PR to `main` (see [`.github/workflows/ci.yml`](.github/workflows/ci.yml)). Run them locally first so the build stays green:

```bash
cargo fmt --all --check               # formatting
cargo clippy --all-targets -- -D warnings   # lints (warnings are errors)
cargo test --all                      # unit + integration tests
```

`cargo audit` also runs in CI to flag vulnerable dependencies. You can run it locally with `cargo install cargo-audit && cargo audit`.

## Guidelines

- **Add tests** for any new feature or behavior change. Unit tests live in `src/lib.rs`; HTTP/integration tests live in `tests/api_tests.rs`.
- **Keep it formatted and lint-clean.** Run `cargo fmt` and fix all clippy warnings before pushing.
- **Use the latest stable dependencies** when adding new crates.
- **Update the docs.** If your change is user-facing, update `README.md` (configuration, API, security, or CLI sections as appropriate).
- **Keep commits focused** and write clear commit messages describing the change.

## Pull-request flow

1. Fork and branch from `main`.
2. Make your change with accompanying tests and docs.
3. Ensure `fmt`, `clippy`, and `test` all pass locally.
4. Open a pull request against `main`. CI must pass before merge.

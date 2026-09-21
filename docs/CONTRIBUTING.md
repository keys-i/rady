# Contributing

Install Rust 1.85 or newer. Keep changes focused and add a compact table-driven test when behaviour changes.

Run the local release gate:

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --no-fail-fast --locked
cargo build --release --locked
```

Use `cargo fmt` to apply formatting. For a focused check, select the module or test name, for example `cargo test quality::tests`.

Workflow changes should also pass `actionlint .github/workflows/*.yml` when `actionlint` is installed. The reusable review workflow is for private repositories on trusted self-hosted runners with an authenticated native harness. Never add subscription credentials or model API keys to public workflows.

Use Conventional Commit pull-request titles. `fix:` produces a patch release, `feat:` a minor release, and `!` or `BREAKING CHANGE:` a major release. Release Please updates `Cargo.toml` and [CHANGELOG.md](CHANGELOG.md), then creates the tag and release after its release pull request is merged.

Contributions are accepted under the [MIT License](../LICENSE).

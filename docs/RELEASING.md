# Releasing Rady

Release Please owns future releases. `Cargo.toml`, `Cargo.lock`, and `tools/config/release-manifest.json` must agree.

The current publishable crate version is `0.5.6`. This document does not claim that `v0.5.6` or the crate has already been released.

When Release Please creates a GitHub release from the merged release pull request, the release workflow checks out that exact tag and runs `cargo publish --locked`. It needs `CARGO_REGISTRY_TOKEN` as a secret in `keys-i/rady`. Crates.io publishing is therefore tied to the corresponding future GitHub release, not performed by a local backfill script.

## Historical releases

GitHub releases from `v0.1.0` through `v0.5.5` were reconstructed from the first default-branch commit carrying each recorded version. They retain their real publication dates; no tags were rewritten.

Historical crates.io releases cannot be fabricated or backfilled: crates.io versions are immutable and need the verified source artifact for that version. The same restriction applies to historical package-manager artifacts.

## Before merging a release PR

- Check `Cargo.toml`, the `rady` entry in `Cargo.lock`, and the release manifest agree
- Run formatting, Clippy, all tests, and the locked release build
- Confirm release notes and installation documentation name the intended version
- Confirm `CARGO_REGISTRY_TOKEN` is available only to the central release workflow

# Releasing Rady

`Cargo.toml` and the `rady` entry in `Cargo.lock` name the next package version: `0.6.0`. The release manifest, changelog, and Homebrew formula record only completed releases. They intentionally differ until a tagged release exists.

`0.6.0` is prepared, not released. Release Please owns the release pull request, manifest, changelog, tag, and GitHub release. The release workflow checks out that exact tag and runs `cargo publish --locked`. After the tag exists, update the Homebrew formula to that tag and verify installation. `CARGO_REGISTRY_TOKEN` belongs only in the central `keys-i/rady` release workflow.

## Historical releases

GitHub releases from `v0.1.0` through `v0.5.5` were reconstructed from the first default-branch commit carrying each recorded version. They retain their real publication dates; no tags were rewritten.

Historical crates.io releases cannot be fabricated or backfilled: crates.io versions are immutable and need the verified source artifact for that version. The same restriction applies to historical package-manager artifacts.

## Before merging

- Confirm `Cargo.toml` and `Cargo.lock` say the intended next version
- Confirm the manifest says the latest actual release
- Run formatting, Clippy, all tests, and the locked release build
- Confirm release notes and installation documentation name the intended version
- Confirm `CARGO_REGISTRY_TOKEN` is available only to the central release workflow

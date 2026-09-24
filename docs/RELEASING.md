# Releasing Rady

`Cargo.toml`, `Cargo.lock`, the release manifest, changelog, and Homebrew formula must name the same release before its tag is created.

Release Please normally owns the release pull request, tag, GitHub release, and locked crates.io publication. Manual recovery releases must preserve the same order and publish the exact tagged source. `CARGO_REGISTRY_TOKEN` belongs only in the central `keys-i/rady` release workflow or the maintainer's local Cargo credential store.

## Historical releases

GitHub releases through `v0.5.8` were reconstructed from the exact default-branch commits carrying each version. Their tags were not rewritten.

An unpublished historical crates.io version may be recovered only from its verified source commit. Published crates are immutable; never rebuild an old version from newer source or move an existing tag.

## Before merging

- Confirm `Cargo.toml` and `Cargo.lock` say the intended next version
- Confirm the manifest and Homebrew formula name the intended tag
- Run formatting, Clippy, all tests, and the locked release build
- Confirm release notes and installation documentation name the intended version
- Confirm `CARGO_REGISTRY_TOKEN` is available only to the central release workflow

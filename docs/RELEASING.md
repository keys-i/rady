# Releasing Rady

`Cargo.toml`, `Cargo.lock`, the release manifest, changelog, and Homebrew formula must name the same release before its tag is created.

Release Please normally owns the release pull request, tag, GitHub release, and locked crates.io publication. `CARGO_REGISTRY_TOKEN` belongs only in the central `keys-i/rady` release workflow or the maintainer's local Cargo credential store. `RADY_RELEASE_TOKEN` must be a fine-grained token limited to `keys-i/rady` with Contents, Issues and Pull requests write access so release pull requests trigger their checks; it must never be copied to an installed repository.

## Recovering a crates.io publication

Use **Actions → Release → Run workflow** only after the matching GitHub release already exists. Enter its exact `vX.Y.Z` tag. The workflow rejects anything else, checks out that tag without credentials, confirms `Cargo.toml` contains the matching `rady` version, and checks crates.io first. A version already published is a successful no-op; an absent version is published from that immutable checkout. It never publishes `main` during recovery.

## Historical releases

GitHub releases through `v0.5.8` were reconstructed from the exact default-branch commits carrying each version. Their tags were not rewritten.

An unpublished historical crates.io version may be recovered only with the recovery workflow and its existing GitHub release tag. Published crates are immutable; never rebuild an old version from newer source or move an existing tag.

Every immutable crate and tag must come from its own verified, versioned source state. Never rebuild an older version from the current tree or move an existing tag. This document does not claim that any external release exists.

## Before merging

- Confirm `Cargo.toml` and `Cargo.lock` say the intended next version
- Confirm the manifest and Homebrew formula name the intended tag
- Run formatting, Clippy, all tests, and the locked release build
- Confirm release notes and installation documentation name the intended version
- Confirm `RADY_RELEASE_TOKEN` and `CARGO_REGISTRY_TOKEN` are available only to the central release workflow

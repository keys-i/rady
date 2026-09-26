# Releasing Pekin

`Cargo.toml`, `Cargo.lock`, the release manifest, changelog, and Homebrew formula must name the same release before its tag is created.

## 0.6.8 release notes

0.6.8 completes the Pekin rename and adds hosted approved-write dispatch: an explicit request and same-author approval can create an isolated branch and pull request through the pinned, file-only Gemini harness and a short-lived repository-scoped token. The service never merges. Mention and review requests send bounded evidence; approved hosted edits may send repository files selected by the constrained provider CLI. Terms and Privacy move to `2026-09-26-t3` and `2026-09-26-p3`; connected repositories must install the Pekin App and run setup again before hosted processing resumes.

Release Please normally owns the release pull request, tag, GitHub release, and locked crates.io publication. `CARGO_REGISTRY_TOKEN` belongs only in the central `keys-i/rady` release workflow or the maintainer's local Cargo credential store. `PEKIN_RELEASE_TOKEN` must be a fine-grained token limited to `keys-i/rady` with Contents, Issues and Pull requests write access so release pull requests trigger their checks; it must never be copied to an installed repository.

## Recovering a crates.io publication

Use **Actions → Release → Run workflow** only after the matching GitHub release already exists. Enter its exact `vX.Y.Z` tag. The workflow rejects anything else, checks out that tag without credentials, confirms `Cargo.toml` contains the matching `pekin` version, and checks crates.io first. A version already published is a successful no-op; an absent version is published from that immutable checkout. It never publishes `main` during recovery.

## Historical releases

The [changelog](CHANGELOG.md) starts at Rady 0.1.0 and ends at the current Pekin 0.6.8 version. Its early entries summarize the packaged artifacts; they do not certify the state of remote tags or registries.

An unpublished historical crates.io version may be recovered only with the recovery workflow and its existing GitHub release tag. Published crates are immutable; never rebuild an old version from newer source or move an existing tag.

Every immutable crate and tag must come from its own verified, versioned source state. Never rebuild an older version from the current tree or move an existing tag.

## Before merging

- Confirm the public GitHub App uses the `pekin` slug and the central repository has the required `PEKIN_*` secrets and variables
- Confirm `Cargo.toml` and `Cargo.lock` say the intended next version
- Confirm the manifest and Homebrew formula name the intended tag
- Run formatting, Clippy, all tests, and the locked release build
- Confirm release notes and installation documentation name the intended version
- Confirm the consent constants match the Terms and Privacy versions named in the release notes
- Confirm `PEKIN_RELEASE_TOKEN` and `CARGO_REGISTRY_TOKEN` are available only to the central release workflow

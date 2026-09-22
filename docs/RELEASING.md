# Releasing Rady

Rady uses Release Please. Version state lives in `Cargo.toml`, `Cargo.lock` and `tools/config/release-manifest.json`; they must agree after a release is published.

## Retrospective releases

GitHub releases from `v0.1.0` through `v0.5.5` were reconstructed from the first commit of each version recorded in the default branch's `Cargo.toml` history. Only versions that actually existed were published; no tag was rewritten. GitHub records their real publication date rather than a fabricated historical date.

The backfill covers GitHub tags and releases only. It does not retroactively publish crates.io packages or historical Homebrew formulae, because those channels are immutable and require separately verified source artifacts. The current Homebrew formula targets the current release tag.

## Current and future releases

`tools/config/release-manifest.json` records `0.5.5` as the released baseline. Future conventional changes flow through the Release Please pull request and release workflow. Do not manually edit the manifest for ordinary releases.

Before merging a release pull request, verify:

- `Cargo.toml` and the `rady` package entry in `Cargo.lock` have the same version
- the release manifest and changelog match the intended release
- formatting, Clippy, all tests and the release build pass with `--locked`
- the Homebrew formula points to the intended immutable tag or artifact
- installation documentation names the published version

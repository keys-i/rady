# Releasing Rady

Rady uses Release Please for future releases. Version `0.5.5` bootstraps that history with a guarded one-shot script for earlier releases. Version state lives in `Cargo.toml`, `Cargo.lock` and `tools/config/release-manifest.json`; they must agree after a release is published.

## Retrospective releases

`tools/config/release-backfill.json` bounds the retrospective series from `0.1.0` through `0.5.5`. `.github/workflows/scripts/backfill-releases.sh` discovers each version transition recorded in the default branch's first-parent `Cargo.toml` history and selects the first commit where that version appeared. It rejects missing boundaries and repeated version segments while planning, then rejects unreachable commits and existing tags that point somewhere else before publication.

From a clean `main` checkout, review the plan before publishing:

```sh
.github/workflows/scripts/backfill-releases.sh
```

Then authenticate GitHub CLI and publish directly:

```sh
GH_TOKEN="$(gh auth token)" RADY_RELEASE_CONFIRM=BACKFILL \
  .github/workflows/scripts/backfill-releases.sh --apply
```

The script creates only missing tags and GitHub releases, processes them oldest first and never rewrites an existing tag or release. Remove the script and `tools/config/release-backfill.json` after every release succeeds; the backfill is complete and must not be recreated. GitHub records the actual backfill date as the publication date; that date cannot be made historical.

The backfill covers GitHub tags and releases only. It does not retroactively publish crates.io packages or historical Homebrew formulae, because those channels are immutable and require separately verified source artifacts. The current Homebrew formula targets the current release tag.

## Current and future releases

After the backfill has published `v0.5.5`, `tools/config/release-manifest.json` correctly records `0.5.5` as the released baseline. Until that release exists, the regular release workflow skips Release Please so it cannot mistake the repository's full history for unreleased work. Run the **Release** workflow once after a successful backfill to process any commits that arrived while this gate was closed. Future conventional changes flow through the Release Please pull request and release workflow. Do not manually edit the manifest for ordinary releases.

Before merging a release pull request, verify:

- `Cargo.toml` and the `rady` package entry in `Cargo.lock` have the same version
- the release manifest and changelog match the intended release
- formatting, Clippy, all tests and the release build pass with `--locked`
- the Homebrew formula points to the intended immutable tag or artifact
- installation documentation names the published version

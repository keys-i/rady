# Releasing Koelu

Koelu 0.6.9 is the first version under the new name. `Cargo.toml` and `Cargo.lock` describe that candidate; the release manifest remains at the last released version, 0.6.8, until Release Please advances it. Keep the crate, formula, changelog, and release tag aligned before publication.

## Cutover checks

- Confirm the public [Koelu App](https://github.com/apps/koelu) is owned by `keys-i`; verify its visibility and exact permissions, and confirm the crate name is available before publishing
- Move the central repository's existing `RADY_*` credentials to `KOELU_*` names, including `KOELU_RELEASE_TOKEN`, without copying them to installed repositories; the Koelu App ID and slug variables are already set
- Confirm Terms `2026-09-27-t4` and Privacy `2026-09-27-p4` match the code and setup preview; each installed repository must accept them again
- Run formatting, Clippy, tests, the locked release build, and a dry-run package check
- Confirm the Koelu formula points at the eventual `v0.6.9` tag; the old Rady formula stays pinned to its historical tag and names Koelu as the replacement

Release Please normally creates the release pull request, tag, GitHub release, and locked crates.io publication. Add `CARGO_REGISTRY_TOKEN` and `KOELU_RELEASE_TOKEN` to the central `keys-i/rady` repository before publication. The latter needs only the repository permissions required to create a release pull request and trigger its checks.

## Recovering publication

Use **Actions → Release → Run workflow** only when the matching Koelu GitHub release and immutable `vX.Y.Z` tag already exist. The workflow checks out that tag, verifies its crate name and version, checks crates.io, and publishes only if that exact version is absent. Do not rebuild an older release from current source or move an existing tag.

The [changelog](CHANGELOG.md) records the Rady and Pekin years under their original names. Existing `rady` and `pekin` crates do not automatically redirect users to Koelu; the Rady Homebrew formula is a migration notice, while the Pekin formula is removed. The [upgrade guide](UPGRADING.md) covers installed CLIs and repository consent.

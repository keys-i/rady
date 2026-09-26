# Releasing Koelu

Koelu 0.6.9 was the first version under the new name. Keep the crate, release manifest, formula, changelog, and release tag aligned before publication.

Publish the repository-name cutover in Koelu 0.6.10 before renaming `keys-i/rady` to `keys-i/koelu`: 0.6.9 checks the old name exactly. After the rename, rerun setup and update the committed source pin and consent receipt.

## Cutover checks

- Confirm the public [Koelu App](https://github.com/apps/koelu) is owned by `keys-i`; verify its visibility and exact permissions, and confirm the crate name is available before publishing
- Move the central repository's existing `RADY_*` credentials to `KOELU_*` names, including `KOELU_RELEASE_TOKEN`, without copying them to installed repositories; the Koelu App ID and slug variables are already set
- Confirm Terms `2026-09-27-t4` and Privacy `2026-09-27-p4` match the code and setup preview; each installed repository must accept them again
- Run formatting, Clippy, tests, the locked release build, and a dry-run package check
- Confirm the Koelu formula points at the eventual `v0.6.10` tag; the old Rady formula stays pinned to its historical tag and names Koelu as the replacement

Release Please normally creates the release pull request, tag, GitHub release, and locked crates.io publication. Add `CARGO_REGISTRY_TOKEN` and `KOELU_RELEASE_TOKEN` to the central `keys-i/koelu` repository before publication. The latter needs only the repository permissions required to create a release pull request and trigger its checks.

## Recovering publication

Use **Actions → Release → Run workflow** only when the matching Koelu GitHub release and immutable `vX.Y.Z` tag already exist. The workflow checks out that tag, verifies its crate name and version, checks crates.io, and publishes only if that exact version is absent. Do not rebuild an older release from current source or move an existing tag.

The [changelog](CHANGELOG.md) records the Rady releases under their original name. The unpublished Pekin candidate is included in Koelu 0.6.9. The notice-only `rady` crate and deprecated Homebrew formula point to Koelu; they do not automatically move installed CLIs or saved runs. The [upgrade guide](UPGRADING.md) covers installed CLIs and repository consent.

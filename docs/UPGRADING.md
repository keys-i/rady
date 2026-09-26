# Upgrading to Koelu

Koelu replaces Pekin, which replaced Rady. You can upgrade directly from any earlier release. The [changelog](CHANGELOG.md) keeps the old names and commands in their historical entries.

## Install the new CLI

Install Koelu with Cargo or the Homebrew tap:

```sh
cargo install koelu --locked
koelu --help
```

```sh
brew tap keys-i/koelu https://github.com/keys-i/koelu
brew install keys-i/koelu/koelu
```

You can remove the previous CLI after Koelu works on your machine with `cargo uninstall pekin` or `cargo uninstall rady`; for Homebrew, uninstall the matching old formula. The old local run folders are not moved. Keep any evidence you still need before removing them.

## Move the hosted service

The public GitHub App and mention must both use **Koelu** and `@koelu`. Before deploying the renamed central workflow:

1. Confirm the existing App is [Koelu](https://github.com/apps/koelu) under `keys-i`, with its intended visibility and permissions.
2. In the central `keys-i/koelu` repository only, move the existing `RADY_*` credentials to their `KOELU_*` names. `KOELU_APP_CLIENT_ID` and `KOELU_APP_SLUG=koelu` are already set. Re-enter the App private key, Gemini key, and Cerebras key as Koelu secrets through GitHub settings; GitHub does not reveal their old values. Configure any additional provider keys you use, and add `KOELU_RELEASE_TOKEN` and `CARGO_REGISTRY_TOKEN` before publishing. Do not put these in target repositories.
3. Deploy the Koelu central workflow only after the App and credentials are ready. Old environment variable names are not read by this version.
4. Run setup as an administrator in each connected repository, then review and commit its generated `.github/koelu.json`. Remove an obsolete `.github/pekin.json` only after the new setup succeeds and its contents are no longer needed.

Until the new workflow is deployed, the existing workflow may still read `RADY_APP_SLUG`. Set that legacy variable's value to `koelu` if you need it to keep running during the cutover; remove the variable after the old workflow is retired.

```sh
koelu setup --repo owner/repo --check test --accept-terms
```

The Koelu service agreement uses Terms `2026-09-27-t4` and Privacy `2026-09-27-p4`. Earlier receipts do not cover this version. Setup records a new, closed consent issue and updates the repository's public configuration. `koelu dependasolve --repo owner/repo --check test --apply --accept-terms` is the scriptable equivalent; it configures the repository but does not run a review immediately.

After the repository becomes `keys-i/koelu`, use Koelu 0.6.10 or newer for setup. Koelu 0.6.9 still requires the previous trusted repository name.

## What stays the same

The central service polls rather than replying instantly. A mention can ask a read-only question:

```text
@koelu What changed here, and what should I check?
```

An explicit change request still requires the same author to approve the exact proposal before Koelu creates an isolated branch and pull request. Koelu never merges for you. The `keys-i/koelu` source pin, selected checks, and consent receipt remain required for hosted work; changing the product name does not loosen those gates.

<h1 align="center">Rady</h1>

<p align="center">
  <a href="https://github.com/keys-i/rady/actions/workflows/checks.yml"><img alt="Checks" src="https://github.com/keys-i/rady/actions/workflows/checks.yml/badge.svg"></a>
  <a href="https://crates.io/crates/rady"><img alt="Crates.io" src="https://img.shields.io/crates/v/rady.svg"></a>
  <a href="https://crates.io/crates/rady"><img alt="Downloads" src="https://img.shields.io/crates/d/rady.svg"></a>
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/github/license/keys-i/rady"></a>
</p>

<p align="center">
  <img src="docs/assets/rady-poster.webp" alt="Rady's duck at a terminal" width="760">
</p>

Rady connects a repository to RadDuck: a small GitHub service that answers `@radduck` and reviews eligible Dependabot pull requests. You keep the final merge.

## Install

```sh
cargo install rady --locked
rady --help
```

With Homebrew:

```sh
brew tap keys-i/rady https://github.com/keys-i/rady
brew install keys-i/rady/rady
```

Or build a checkout with Rust 1.85+:

```sh
cargo build --release --locked
target/release/rady --help
```

## Connect a repository

1. Install the **RadDuck** GitHub App for the repository.
2. Read the [Terms](docs/TERMS.md) and [Privacy policy](docs/PRIVACY.md).
3. Run setup as a repository administrator:

```sh
rady setup --repo owner/repo --check test --accept-terms
```

Setup previews what it will do, writes a small public `.github/rady.json`, creates a closed consent receipt, and adds Dependabot configuration only when it is missing. It never copies service credentials into the repository or changes branch protection. Review and commit the generated files.

`rady dependasolve` is the scriptable setup form. It is not a direct review command:

```sh
rady dependasolve --repo owner/repo --check test --apply --accept-terms
```

Omit `--solver-ref` to use the source setup resolves; provide `keys-i/rady@40_CHARACTER_COMMIT_SHA` only when deliberately holding a known release.

## What happens after setup

The central `keys-i/rady` service polls a bounded recent window of consented installations. Mentions and discovery use short-lived, installation-scoped tokens with only the permissions needed for that operation. Each selected review receives a repository-scoped token.

Review windows rotate fairly across eligible pull requests. Rady honours the configured source pin, verifies Dependabot evidence, reads the selected CI checks, and leaves unsupported, grouped, or ambiguous updates as a `COMMENT` for human review. The public App has read-only contents access. It does not enable, disable, or perform merges; a person decides whether to merge.

Ask in an issue or pull request:

```text
@radduck What changed here, and what should I check?
```

Replies are concise, evidence-based, and read-only. Polling is not real time; work begins on a later service cycle.

## Work locally

```sh
rady code "fix the parser" --check test
rady agent ask "where is this parser called?"
rady agent follow-up RUN_ID "which failure should I fix first?"
```

Use `rady runs`, `inspect`, `cancel`, `resume`, and `apply` to control retained work. Rady can load repository guidance, selected skills and MCP servers; its terminal reports render Markdown and mathematics with accessible themes.

## Safety

Service credentials and model keys stay only in `keys-i/rady`. Rady bounds model evidence, rejects unaccepted repositories, and treats issue text, diffs, and model output as untrusted. Hosted models may receive bounded evidence only under the configured provider policy; see [Privacy](docs/PRIVACY.md).

[Upgrading](docs/UPGRADING.md) · [Releasing](docs/RELEASING.md) · [Security](docs/SECURITY.md) · [Terms](docs/TERMS.md) · [Privacy](docs/PRIVACY.md) · [MIT](LICENSE)

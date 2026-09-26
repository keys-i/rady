<h1 align="center">Koelu</h1>

<p align="center">
  <a href="https://github.com/keys-i/koelu/actions/workflows/checks.yml"><img alt="Checks" src="https://github.com/keys-i/koelu/actions/workflows/checks.yml/badge.svg"></a>
  <a href="https://crates.io/crates/koelu"><img alt="Crates.io" src="https://img.shields.io/crates/v/koelu.svg"></a>
  <a href="https://crates.io/crates/koelu"><img alt="Downloads" src="https://img.shields.io/crates/d/koelu.svg"></a>
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/github/license/keys-i/koelu"></a>
</p>

<p align="center">
  <img src="docs/assets/koelu-poster.webp" alt="Koelu's duck at a terminal" width="760">
</p>

Koelu is a small GitHub service for repositories: it answers `@koelu` and reviews eligible Dependabot pull requests. You keep the final merge.

Koelu takes its name from the koel, an Indian songbird. The duck stays at the keyboard. Say it “KOH-loo.”

## Install

```sh
cargo install koelu --locked
koelu --help
```

With Homebrew:

```sh
brew tap keys-i/koelu https://github.com/keys-i/koelu
brew install keys-i/koelu/koelu
```

Coming from Rady or Pekin? Follow the [migration steps](docs/UPGRADING.md).

Or build a checkout with Rust 1.85+:

```sh
cargo build --release --locked
target/release/koelu --help
```

## Connect a repository

1. Install the **Koelu** GitHub App for the repository.
2. Read the [Terms](docs/TERMS.md) and [Privacy policy](docs/PRIVACY.md).
3. Run setup as a repository administrator:

```sh
koelu setup --repo owner/repo --check test --accept-terms
```

Setup previews what it will do, writes a small public `.github/koelu.json`, creates a closed consent receipt, and adds Dependabot configuration only when it is missing. It never copies service credentials into the repository or changes branch protection. Review and commit the generated files.

`koelu dependasolve` is the scriptable setup form. It is not a direct review command:

```sh
koelu dependasolve --repo owner/repo --check test --apply --accept-terms
```

Omit `--solver-ref` to use the source setup resolves; provide `keys-i/koelu@40_CHARACTER_COMMIT_SHA` only when deliberately holding a known release.

## What happens after setup

The central `keys-i/koelu` service polls a bounded recent window of consented installations. Mentions and discovery use short-lived, installation-scoped tokens with only the permissions needed for that operation. Each selected review receives a repository-scoped token.

Review windows rotate fairly across eligible pull requests. Koelu honours the configured source pin, verifies Dependabot evidence, reads the selected CI checks, and leaves unsupported, grouped, or ambiguous updates as a `COMMENT` for human review. Koelu reserves Contents write access for an approved coding path, but mention and review tokens are explicitly narrowed to read-only. They cannot push or merge.

Ask in an issue or pull request:

```text
@koelu What changed here, and what should I check?
```

Replies are concise, evidence-based, and read-only. Polling is not real time; work begins on a later service cycle.

## Ask for a change

Write requests are deliberate. Start an issue or pull-request comment with `@koelu` and the exact change you want. Koelu records the request, base branch, and base commit, then asks the same person to approve that request by comment ID:

```text
@koelu fix the parser error for empty package names
@koelu approve 123456789
```

Before it does any write work, Koelu rechecks the open conversation, the same-author approval, the recorded request and base revision, and that the author still has write, maintain, or admin access. It uses a short-lived token scoped to that repository, checkpoints each completed step on an isolated `koelu/...` branch, and opens one pull request. It never merges.

If the base changes, an approval is invalid, access has changed, or checks fail, Koelu stops without changing the default branch. It posts a result when the run finishes; a later service pass marks abandoned claims expired after two hours when the bounded comment scan can find them. Claimed requests are never retried automatically. If a claim has no result, inspect the service run before making a fresh request and approval.

## Work locally

```sh
koelu code "fix the parser" --check test
koelu agent ask "where is this parser called?"
koelu agent follow-up RUN_ID "which failure should I fix first?"
```

Use `koelu runs`, `inspect`, `cancel`, `resume`, and `apply` to control retained work. Koelu can load repository guidance, selected skills and MCP servers; its terminal reports render Markdown and mathematics with accessible themes.

## Safety

App credentials stay only in `keys-i/koelu`; they never enter child processes. Mentions and reviews send bounded evidence to their selected provider. An approved hosted edit runs a constrained provider CLI that may read and send repository files it selects for that task; its selected provider API key reaches only that scrubbed client child, never the target repository, prompt, or logs. Hosted editing stops before launch when a repository contains `.gemini`, `.env`, or `GEMINI.md`, because the harness would otherwise load target-controlled configuration. See [Privacy](docs/PRIVACY.md).

## Documentation

[Upgrade to Koelu 0.6.9](docs/UPGRADING.md) from Rady or Pekin. The [changelog](docs/CHANGELOG.md) keeps the earlier names as history, not current setup instructions.

For the hosted service, read the current [Terms](docs/TERMS.md) and [Privacy policy](docs/PRIVACY.md). To help with the project, see [Contributing](.github/CONTRIBUTING.md), the [Code of Conduct](.github/CODE_OF_CONDUCT.md), and [Security](.github/SECURITY.md). Maintainers have a [release guide](docs/RELEASING.md). Source code is [MIT licensed](LICENSE).

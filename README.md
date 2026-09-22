# Rady

[![Checks](https://github.com/keys-i/rady/actions/workflows/checks.yml/badge.svg)](https://github.com/keys-i/rady/actions/workflows/checks.yml)
[![Crates.io](https://img.shields.io/crates/v/rady.svg)](https://crates.io/crates/rady)
[![Downloads](https://img.shields.io/crates/d/rady.svg)](https://crates.io/crates/rady)
[![License](https://img.shields.io/github/license/keys-i/rady)](LICENSE)

Rady is a Rust CLI for making checked code changes and reviewing dependency pull requests. You review the evidence and keep the final decision.

- `rady code` turns a request into an isolated, checked change
- `rady dependasolve` reviews Dependabot pull requests from their diff and completed CI evidence

By default, Rady prints concise terminal output and keeps a self-contained HTML report with safe Markdown, local mathematics, responsive type, themes, and a duck illustration. Automation can use `--output json`.

## Install

Install Rady on macOS or Linux from the project tap:

```sh
brew tap keys-i/rady https://github.com/keys-i/rady
brew install keys-i/rady/rady
```

Or install the tagged source with Cargo:

```sh
cargo install --git https://github.com/keys-i/rady --tag v0.5.5 --locked rady
```

Rady needs Rust 1.85+ to build from source. Coding runs need an authenticated [Codex CLI](https://developers.openai.com/codex/cli/) or [Claude Code](https://docs.anthropic.com/en/docs/claude-code); GitHub setup and pull-request review need the GitHub CLI.

```sh
cargo build --release --locked
target/release/rady --help
```

`dependasolver` remains a compatibility command for `rady dependasolve`.

## Make a checked change

For a simple change:

```sh
rady code "Reject blank user names"
```

Rady works in isolation, limits scope, runs the selected checks, and gets a separate read-only review. The workspace and evidence survive a stopped run. Publishing a pull request requires `--pr`, an explicit repository, and fixed acceptance checks.

For repeatable work, pass a JSON specification:

```sh
rady code --spec spec.json --directory /path/to/project
```

Acceptance files must already exist and remain unchanged. Rady parses commands into arguments rather than running a shell. Use `--help` for every option.

Inspect, stop, resume, or apply a retained run:

```sh
rady runs
rady inspect RUN_ID
rady cancel RUN_ID
rady resume RUN_ID
rady apply RUN_ID --directory /path/to/project
```

`apply` verifies the patch and target revision. It never stages, commits, or publishes your work.

## Review dependency pull requests

Preview setup, then apply it when it looks right:

```sh
rady dependasolve --repo OWNER/REPO --check test --check audit
rady dependasolve --repo OWNER/REPO --check test --check audit --apply
```

Setup pins Rady to an immutable commit and replaces only its generated caller files. Existing Dependabot settings and branch protection stay untouched. `--check` names CI evidence to read; it does not run a command or add a required status check.

Dependasolve reviews eligible pull requests oldest first. Passing low-risk updates may be approved; missing evidence holds them. Eligible conflicted Dependabot updates can become separate, reviewed replacement pull requests. Rady never force-pushes, closes the original, or merges for you.

Private repositories use a trusted self-hosted runner. Public repositories admit only trusted same-repository Dependabot or collaborator pull requests after preflight. See [Security](docs/SECURITY.md) for the full boundary.

## Ask Rady on GitHub

Repository owners, members, and collaborators can ask a read-only question on an issue or pull request:

```text
@radyybot What changed here, and what should I check?
```

`@radyybot` alone shows concise usage. Mentions cannot edit code or publish changes. Read [Upgrading](docs/UPGRADING.md) for configuration and [Security](docs/SECURITY.md) before sending repository content to a model provider.

## Trust, output, and themes

Rady keeps generated work distinct from verified work, bounds subprocess time and output, removes common model and GitHub tokens from workers, and fails closed when evidence is missing. Custom adapters are privileged local programs, not sandboxes.

Reports contain no scripts, CDN, remote fonts, or raw untrusted HTML. Markdown and mathematics remain selectable text. `--theme auto` follows the environment; `NO_COLOR`, redirected output, keyboard focus, and reduced motion remain first-class.

[Upgrading](docs/UPGRADING.md) · [Releasing](docs/RELEASING.md) · [Security](docs/SECURITY.md) · [Contributing](docs/CONTRIBUTING.md) · [MIT](LICENSE)

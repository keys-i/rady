<h1 align="center">Rady</h1>

<p align="center">
  <a href="https://github.com/keys-i/rady/actions/workflows/checks.yml"><img alt="Checks" src="https://github.com/keys-i/rady/actions/workflows/checks.yml/badge.svg"></a>
  <a href="https://crates.io/crates/rady"><img alt="Crates.io" src="https://img.shields.io/crates/v/rady.svg"></a>
  <a href="https://crates.io/crates/rady"><img alt="Downloads" src="https://img.shields.io/crates/d/rady.svg"></a>
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/github/license/keys-i/rady"></a>
</p>

<p align="center">
  <img src="docs/assets/rady-poster.webp" alt="Rady duck at a terminal beside the words: Make the change. Check the work. Rady keeps the patch, tests, and review together." width="100%">
</p>

Rady is a Rust CLI for checked changes, dependency reviews, and useful GitHub answers. It keeps the evidence close; you keep the final call.

- `rady code` makes an isolated, checked change
- `rady dependasolve` reviews Dependabot work from its diff and completed CI
- `@radyybot` gives a short, natural answer grounded in the issue or pull request
- `rady agent` brings those flows together with local skills, MCP, memory, and follow-ups

Terminal output is concise. Each run keeps a self-contained HTML report with safe Markdown, local maths, themes, and selectable text. Automation can use `--output json`.

## Install

```sh
brew tap keys-i/rady https://github.com/keys-i/rady
brew install keys-i/rady/rady
```

Install the latest published crate with:

```sh
cargo install rady --locked
```

Build a checkout with Rust 1.85+:

```sh
cargo build --release --locked
target/release/rady --help
```

Code runs need an authenticated [Codex CLI](https://developers.openai.com/codex/cli/) or [Claude Code](https://docs.anthropic.com/en/docs/claude-code). GitHub setup needs `gh`. `dependasolver` remains a compatibility command for `rady dependasolve`.

## Work with an agent

Rady's agent surface stays local by default. It reads bounded project guidance from `AGENTS.md` and `DESIGN.md`, loads explicitly listed skills, and can connect selected local stdio MCP servers.

```sh
rady agent ask "Why does this parser reject empty input?"
rady agent follow-up RUN_ID "Which test proves that?"
```

Read-only questions use the repository in place under a read-only harness; they don’t create an edit worktree. Their bounded conversation memory contains questions and answers, never credentials or private model reasoning. Write tasks keep the existing isolated worktree. Remote `--pr` delivery records a checkpoint per completed task; `--ghost` keeps one final verified commit instead.

Skills and MCP are configured explicitly in `.rady/context.json`. Merely opening a repository never starts a server:

```json
{
  "schema": 1,
  "skills": [".rady/skills/rust.md"],
  "mcp_servers": {
    "docs": { "command": "docs-mcp", "args": [] }
  }
}
```

Select a server only for a write run with `rady code "..." --mcp docs`. Rady passes no model, GitHub, or App keys to it.

Rady resolves obvious intent without a model, sends only ambiguous decisions to the fastest available classifier, and adds an independent evidence brief before deep hosted answers. Known Gemini, Cerebras, and xAI models are tried first; their bounded catalogues are discovered only when a fallback is needed.

The central `keys-i/rady` orchestration workflow is the normal service host. It discovers installed radyybot Apps, mints short-lived installation tokens, and polls consented repositories every five minutes. No workflow or secret is installed in a target repository.

`rady agent serve` is the self-hosted alternative for an operator who needs a continuously running service:

```sh
rady agent serve --app-client-id CLIENT_ID \
  --app-private-key-file /secure/path/radyybot.pem
```

Rady discovers App installations, mints short-lived tokens, and refreshes them in memory. One process covers up to 256 installations; use `--owner OWNER` to narrow or shard a larger deployment. It stops after three failed cycles instead of spinning forever. The scheduled workflow remains a deployment fallback and retains Dependabot’s verified compatibility metadata for protected auto-merge.

## Make a checked change

```sh
rady code "Reject blank user names"
rady code --spec spec.json --directory /path/to/project
```

Rady isolates the work, limits scope, runs your checks, and gets a separate read-only review. A stopped run keeps its workspace and evidence. `--pr` also needs an explicit repository and fixed acceptance checks.

```sh
rady runs
rady inspect RUN_ID
rady cancel RUN_ID
rady resume RUN_ID
rady apply RUN_ID --directory /path/to/project
```

`apply` verifies the patch and target revision; it does not stage, commit, or publish.

## Add radyybot to a repository

This is one guided setup, not a secret-distribution exercise.

1. Install the **radyybot** GitHub App for the repository.
2. Read the [Terms](docs/TERMS.md) and [Privacy policy](docs/PRIVACY.md).
3. Run the guided setup:

```sh
rady setup
```

At a terminal, setup previews the access, files, CI evidence and data handling before asking you to continue. For automation, use `rady setup --repo keys-i/REPO --check test --accept-terms`. Setup writes a small, non-secret `.github/rady.json`, adds Dependabot configuration only when missing, and creates a closed consent receipt. Review and commit the generated files. The central service rechecks the receipt and the signer’s admin access before doing any work.

Replace `keys-i/REPO` with the target repository. That is all a repository administrator configures. The App and model credentials live only in the trusted `keys-i/rady` service. Rady currently polls, so a mention or update is picked up on the next service cycle; real-time responses would require a verified webhook deployment.

`--check` names CI evidence to read. It neither runs a command nor creates a required status check. Dependasolve processes eligible work oldest first and can enable protected auto-merge for a clean, verified update. Conflicted updates remain for local `rady code` repair; the central service never sends its long-lived keys to a self-hosted runner.

`rady dependasolve` remains the compatibility name for this setup flow.

## Ask on GitHub

```text
@radyybot What changed here, and what should I check?
```

Owners, members, and collaborators can use mentions on issues and pull requests. Replies are read-only, concise, and specific to the available evidence; a bare `@radyybot` shows usage. They cannot edit code or publish a change.

GitHub-hosted replies select compatible Gemini and Cerebras models exposed to the configured accounts, then use fallbacks. The catalogs describe accessible models, not a promise of a free tier or unlimited quota; Rady does not evade provider limits. Private repository evidence is sent only to providers explicitly opted in by the central operator.

## Safety

Rady separates generated from verified work, bounds subprocesses and evidence, removes common tokens from worker environments, and fails closed when evidence is missing. Custom adapters are privileged local programs, not sandboxes.

Reports use no scripts, CDNs, remote fonts, or raw untrusted HTML. `--theme auto`, `NO_COLOR`, redirected output, keyboard focus, and reduced motion are all supported.

[Upgrading](docs/UPGRADING.md) · [Releasing](docs/RELEASING.md) · [Security](docs/SECURITY.md) · [Terms](docs/TERMS.md) · [Privacy](docs/PRIVACY.md) · [MIT](LICENSE)

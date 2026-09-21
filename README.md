# Rady

Rady checks agent-made code changes before you apply or publish them. It also reviews Dependabot pull requests using the actual diff and current CI evidence.

One command provides two capabilities:

- `rady code` turns a request into a bounded, checked local change
- `rady dependasolve` reviews dependency pull requests from their real diff and CI evidence

The default interface is for people who code: compact terminal colour, readable Markdown, honest stage progress, and a responsive evidence report. Rady's expressive duck identity and restrained retro-arcade details mark state without covering the work. Automation can select `--output json`; successful `code` and `dependasolve` documents use `{"schema":1,"status":"ok","kind":"…","result":…}` on standard output, while argument and runtime failures use bounded schema-versioned JSON on standard error and retain a nonzero exit. Rady uses an existing Codex or Claude Code login, or an operator-owned command adapter. It provides neither a model nor a hosted service, and it does not call a model API directly.

## Install

Rady supports macOS and Linux. Homebrew installs the pinned source from this repository and provides both `rady` and the `dependasolver` compatibility command:

```sh
brew tap keys-i/rady https://github.com/keys-i/rady
brew install keys-i/rady/rady
```

Cargo can install the same immutable revision directly:

```sh
cargo install --git https://github.com/keys-i/rady \
  --rev 482726baf2c19a02737fa29ec6b94ad068608f3b --locked rady
```

Rady requires an authenticated [Codex CLI](https://developers.openai.com/codex/cli/) or [Claude Code](https://docs.anthropic.com/en/docs/claude-code), unless you configure a custom local adapter. Repository setup and pull-request review also require the GitHub CLI.

## Build

Rady requires Rust 1.85 or newer.

```sh
cargo build --release --locked
target/release/rady --help
```

The compatibility executable `target/release/dependasolver` opens `rady dependasolve`.

## Checked coding changes

Give Rady a request. In a single-language repository it selects the conventional test command; `--check test` requests the same inference explicitly, while a complete command remains available when the repository is unusual:

```sh
rady code "Reject blank user names"
```

Rady creates an isolated worktree from the checked-out revision, asks the selected native harness to plan and implement the change, enforces scope and size limits, runs checks and optional benchmarks, and obtains a separate structured review. It retains the workspace and evidence whether the run succeeds or stops. It publishes only when `--pr`, an explicit repository, and fixed acceptance checks are present.

Use a complete JSON specification to freeze the acceptance contract and skip model planning:

```json
{
  "task": "Reject blank user names",
  "acceptance": [
    "Blank names return a validation error",
    "Valid names keep their existing behaviour"
  ],
  "scope": ["src/validation.rs", "tests/validation.rs"],
  "checks": ["cargo test"],
  "acceptance_checks": [
    {
      "criterion": 0,
      "command": "cargo test blank_name",
      "files": ["tests/validation.rs"]
    },
    {
      "criterion": 1,
      "command": "cargo test valid_name",
      "files": ["tests/validation.rs"]
    }
  ]
}
```

```sh
rady code --spec spec.json --directory /path/to/project
```

Acceptance files must already exist, be regular files, and remain byte-for-byte unchanged. Each criterion needs a fixed check with frozen files, an exact `expected_output`, or both. Commands are parsed into argument lists and run without a shell.

For performance work, add `--benchmark`. Rady records one warm-up and seven samples by default, compares medians and relative median absolute deviation, and blocks noisy or slower results. A benchmark can emit a positive JSON metric such as `{"peak_memory_mb":42.5}` and select it with `--benchmark-metric peak_memory_mb`.

```sh
rady code "Reduce parser allocations" \
  --check "cargo test" \
  --benchmark "target/release/parser fixture.json" \
  --max-regression 3
```

One worker is the efficient default. `--agents 2` through `8` asks the native harness to use subagents. `--max-tokens` is a between-call budget based on provider-reported usage and requires one agent; unknown usage fails closed.

## Human output

`--theme auto` follows the browser colour profile and uses the terminal's own adaptive cyan, so terminal palettes retain control of light/dark contrast. The browser labels `dawn`, `moss`, `tide`, and `dusk` as Paper Tape, Phosphor, Vector, and Midnight; `plain` remains available for unstyled terminal output. Colour is disabled automatically for redirected terminal output and when `NO_COLOR` is set.

Every coding run writes:

- `run.json` for agents and automation
- `run.html` for people, with responsive type, safe Markdown, LaTeX rendered to MathML, accessible theme controls, Rady's expressive duck, restrained terminal detail, and reduced-motion support

Runs stay available after completion or interruption. Use their identifier to inspect evidence, stop active work, restart from a retained patch, or apply a verified result only to a clean directory:

```sh
rady runs
rady inspect RUN_ID
rady cancel RUN_ID
rady resume RUN_ID
rady apply RUN_ID --directory /path/to/project
```

`apply` never stages, commits, or publishes changes. It verifies the retained patch and target revision before modifying the directory.

Markdown headings, **bold**, *italics*, `<u>underline</u>`, `<mark>highlight</mark>`, tables, task lists, footnotes, code, and inline or display mathematics render locally. Other raw HTML is escaped, links are protocol-checked, and remote images become readable text. The report is self-contained: no JavaScript, web font, CDN, or network request. Rady's bundled duck settles into view, shifts occasionally while idle, and responds to nearby controls using only short transform and opacity motion; reduced-motion preferences disable those movements.

## Harnesses

Codex is the default. Claude Code and a custom local command are also supported.

```sh
rady agent doctor --harness codex
rady agent run --harness codex -- resume
```

For a custom adapter, set `RADY_AGENT_COMMAND` and `RADY_REVIEW_COMMAND`. Input arrives on standard input. Review output must match the requested JSON schema. A budgeted adapter result wraps its value and usage:

```json
{"result": {}, "usage": {"input_tokens": 100, "output_tokens": 20}}
```

Custom adapters are privileged local programs, not a sandbox. Rady strips common model and GitHub token variables from worker environments, bounds time and output, and terminates the child process group on overflow or timeout.

## Dependabot pull-request review

Preview repository setup before applying it:

```sh
rady dependasolve \
  --repo OWNER/REPO \
  --check test \
  --check audit \
  --check dependency-review
```

Review the preview, then add `--apply`. Setup resolves the current default-branch commit of `keys-i/rady` and pins that immutable SHA in the workflow. Rerunning setup replaces its generated caller workflow by default; pass `--no-overwrite` to refuse changes, while an existing Dependabot configuration is always left untouched. Pass `--solver-ref keys-i/rady@40_CHARACTER_COMMIT_SHA` only to override that source explicitly. Rady reuses a complete `RADY_APP_*` credential set by default and accepts complete legacy `DEPENDASOLVER_APP_*` credentials as a fallback; `--new-app` explicitly starts a new registration. Setup requires GitHub CLI authentication and repository administration access.

Setup never changes branch protection. Each `--check` tells Rady which completed CI evidence to read; it does not execute that name or create a protected status check. `--checks` remains a compatibility alias. Reviews run on a trusted self-hosted runner. Private repositories review every open, non-draft pull request. Public repositories first verify the pull request on a GitHub-hosted runner and admit only same-repository Dependabot updates or same-repository pull requests from an owner, member, or collaborator, using the bounded Codex or Claude harness. Public forks and custom command adapters are rejected.

Dependasolve approves only low-risk, complete reviews with every selected and protected check passing. It suspends stale Dependabot auto-merge before starting a new review. Auto-merge is restored only when existing branch protection is strict, administrator-enforced, and has at least one required check, after verified patch/minor metadata, no maintainer changes, and 95–100% compatibility. Otherwise Rady posts its evidence and leaves the merge decision with you. Missing evidence holds the change; model confidence never replaces a gate.

Setup discovers local GitHub Actions workflows; their completion and third-party check runs trigger a fresh review of the exact pull-request head. Scheduled and manual runs take a bounded oldest-first pass over every eligible open pull request: all non-drafts in private repositories, or trusted same-repository Dependabot, owner, member, and collaborator pull requests in public repositories. Legacy commit-status checks remain gated but need a manual workflow dispatch after they settle.

For a genuinely conflicted same-repository Dependabot patch or minor update, Rady can ask the native agent harness to recreate that bounded dependency change from the current base. This path requires an approved low-risk diff and selected CI evidence, 95–100% compatibility, no maintainer changes, and only approved manifest, lockfile or workflow paths. The original head and base are pinned and rechecked, and publication stops if the base advances during the run. Rady runs the inferred project test and fixed acceptance check without exposing the App token to either process, then opens a separate replacement PR and links it from the original. It never pushes to or closes the Dependabot PR, and the replacement remains a normal human review and merge decision.

On an issue or pull request, repository owners, members, and collaborators can write `@rady <prompt>` for a guarded, read-only evidence response. `@rady` by itself gives concise usage. Comment requests never edit code, create a change, or publish anything. Mentions from everyone else are treated as untrusted input and cannot start an agent run.

[Upgrading](docs/UPGRADING.md) · [Security](docs/SECURITY.md) · [Contributing](docs/CONTRIBUTING.md) · [MIT](LICENSE)

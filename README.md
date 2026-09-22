# Rady

Rady is a careful duck for code changes and dependency updates. It keeps the person in charge: make a bounded change, see the evidence, then decide what to apply or merge.

- `rady code` turns a request into an isolated, checked change
- `rady dependasolve` reviews Dependabot pull requests from their diff and completed CI evidence

The terminal is compact and direct. The retained HTML report is the readable record: safe Markdown, LaTeX-to-MathML, responsive type, themes, and a restrained retro duck—never a wall of agent theatre. Use `--output json` when another tool is driving Rady.

## Install

Once `v0.5.5` is published, install Rady on macOS or Linux from the project tap:

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

Start with the job, not a ceremony:

```sh
rady code "Reject blank user names"
```

Rady creates an isolated worktree, plans and makes a bounded change, checks scope and size, runs the selected checks, and gets a separate read-only review. It retains the workspace and evidence whether the run succeeds or stops. A pull request requires `--pr`, an explicit repository, and fixed acceptance checks.

For exact, repeatable work, supply a JSON specification:

```json
{
  "task": "Reject blank user names",
  "acceptance": ["Blank names return a validation error"],
  "scope": ["src/validation.rs", "tests/validation.rs"],
  "checks": ["cargo test"],
  "acceptance_checks": [{
    "criterion": 0,
    "command": "cargo test blank_name",
    "files": ["tests/validation.rs"]
  }]
}
```

```sh
rady code --spec spec.json --directory /path/to/project
```

Acceptance files must already exist and remain unchanged. Rady parses commands into arguments rather than running a shell. See [`docs/UPGRADING.md`](docs/UPGRADING.md) for command changes and the built-in `--help` for every option.

Each run leaves a durable record you can inspect, stop, resume, or apply to a clean directory:

```sh
rady runs
rady inspect RUN_ID
rady cancel RUN_ID
rady resume RUN_ID
rady apply RUN_ID --directory /path/to/project
```

`apply` verifies the retained patch and target revision; it never stages, commits, or publishes your work.

## Review dependency pull requests

Preview setup, then apply it when it looks right:

```sh
rady dependasolve --repo OWNER/REPO --check test --check audit
rady dependasolve --repo OWNER/REPO --check test --check audit --apply
```

Rady pins its source to the current immutable default-branch commit. Re-running setup replaces only its generated caller workflow (use `--no-overwrite` to refuse); existing Dependabot configuration and branch protection stay untouched. `--check` names CI evidence to read—it does not run that command or add a required status check.

Dependasolve reviews eligible open pull requests oldest first when triggered manually or on schedule, including existing Dependabot pull requests when there are no new ones. It approves only low-risk updates with the selected evidence complete and passing. Missing evidence holds the change. For an eligible conflicted patch or minor Dependabot update, it can create a separately reviewed replacement pull request; it never force-pushes, closes the original, or merges for you.

Private repositories use a trusted self-hosted runner. Public repositories admit only same-repository Dependabot updates and trusted same-repository collaborator pull requests after GitHub-hosted verification. See the generated workflow and [`docs/SECURITY.md`](docs/SECURITY.md) for the full trust model.

## Ask Rady on GitHub

Owners, members, and collaborators can post a guarded read-only question on an issue or pull request:

```text
@radyybot What changed here, and what should I check?
```

`@radyybot` alone gives concise usage. Mentions never edit code, create a pull request, or publish a change. Untrusted commenters cannot start an agent run.

The hosted responder defaults to Gemini-first, with configured fallbacks. It does not install or sign in to Codex on GitHub Actions. Configure these repository or organisation settings:

| Setting | Name |
| --- | --- |
| Actions secret | `RADY_GEMINI_API_KEY` |
| Optional Actions secret | `RADY_CEREBRAS_API_KEY`, `RADY_XAI_API_KEY` |
| Actions variable | `RADY_GEMINI_PRIVATE_OK=true`, `RADY_XAI_PRIVATE_OK=true` to explicitly allow the provider for private repositories |

Provider prompts contain bounded issue or pull-request evidence. Enable only providers whose data handling and billing are appropriate for that repository; quota and pricing change, so check the provider directly. Hosted replies do not replace the trusted native harness used for code changes, dependency review, or conflict repair.

## Trust, output, and themes

Rady keeps generated work distinct from verified work. It bounds subprocess time and output, strips common model and GitHub tokens from workers, and fails closed when required evidence is missing. Custom adapters are privileged local programs, not sandboxes.

Human reports are self-contained: no script, CDN, remote font, or raw untrusted HTML. Markdown and mathematics remain selectable text; unsafe links and raw HTML are rejected or escaped. `--theme auto` respects the terminal or system context, `NO_COLOR` and redirected output remain plain, and reduced-motion preferences remove decorative movement.

[Upgrading](docs/UPGRADING.md) · [Releasing](docs/RELEASING.md) · [Security](docs/SECURITY.md) · [Contributing](docs/CONTRIBUTING.md) · [MIT](LICENSE)

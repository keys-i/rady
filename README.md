# Rady

Rady is one human-first agentic tool with two capabilities:

- `rady code` turns a request into a bounded, checked local change
- `rady dependasolve` reviews dependency pull requests from their real diff and CI evidence

The default interface is for people who code: compact terminal colour, readable Markdown, honest stage progress, and a responsive evidence report. Rady's common brushtail possum identity and restrained retro-arcade details mark state without covering the work. Automation can select `--output json`; successful `code` and `dependasolve` documents use `{"schema":1,"status":"ok","kind":"…","result":…}` on standard output, while argument and runtime failures use bounded schema-versioned JSON on standard error and retain a nonzero exit. Rady uses an existing Codex or Claude Code login, or an operator-owned command adapter; it does not call a model API directly.

## Build

Rady requires Rust 1.85 or newer.

```sh
cargo build --release --locked
target/release/rady --help
```

The compatibility executable `target/release/dependasolver` opens `rady dependasolve`.

## Code

Give Rady a request and at least one check:

```sh
rady code "Reject blank user names" \
  --directory /path/to/project \
  --check "cargo test"
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
- `run.html` for people, with responsive type, safe Markdown, LaTeX rendered to MathML, accessible theme controls, a zoologically accurate common brushtail silhouette, restrained terminal detail, and reduced-motion support

Runs stay available after completion or interruption. Use their identifier to inspect evidence, stop active work, restart from a retained patch, or apply a verified result only to a clean directory:

```sh
rady runs
rady inspect RUN_ID
rady cancel RUN_ID
rady resume RUN_ID
rady apply RUN_ID --directory /path/to/project
```

`apply` never stages, commits, or publishes changes. It verifies the retained patch and target revision before modifying the directory.

Markdown headings, **bold**, *italics*, `<u>underline</u>`, `<mark>highlight</mark>`, tables, task lists, footnotes, code, and inline or display mathematics render locally. Other raw HTML is escaped, links are protocol-checked, and remote images become readable text. The report is self-contained: no JavaScript, web font, CDN, or network request. The CC0 common brushtail silhouette is by Rachel T Mason via PhyloPic; the interface keeps motion on state and control feedback instead of animating the animal as a cartoon.

## Harnesses

Codex is the default. Claude Code and a custom local command are also supported.

```sh
rady doctor --harness codex
rady agent --harness codex -- resume
```

For a custom adapter, set `RADY_AGENT_COMMAND` and `RADY_REVIEW_COMMAND`. Input arrives on standard input. Review output must match the requested JSON schema. A budgeted adapter result wraps its value and usage:

```json
{"result": {}, "usage": {"input_tokens": 100, "output_tokens": 20}}
```

Custom adapters are privileged local programs, not a sandbox. Rady strips common model and GitHub token variables from worker environments, bounds time and output, and terminates the child process group on overflow or timeout.

## Dependasolve

Preview repository setup before applying it:

```sh
rady dependasolve \
  --repo OWNER/REPO \
  --solver-ref keys-i/dependasolver@40_CHARACTER_COMMIT_SHA \
  --checks test audit dependency-review
```

Review the preview, then add `--apply`. Repeat with `--app rady` to configure the separate Rady review identity. Setup requires GitHub CLI authentication and repository administration access. The reusable workflow accepts only an immutable source SHA and runs account-authenticated agent tooling only on a trusted private self-hosted runner.

Dependasolve approves only low-risk, complete reviews with every configured and protected check passing. It suspends stale Dependabot auto-merge before starting a new review. Auto-merge is restored only after verified patch/minor metadata, no maintainer changes, and 95–100% compatibility. Missing evidence holds the change; model confidence never replaces a gate.

Setup discovers local GitHub Actions workflows; their completion and third-party check runs trigger a fresh review of the exact Dependabot pull-request head. Legacy commit-status checks remain gated but need a manual workflow dispatch after they settle. A held update says whether to wait for CI or gives a bounded maintainer repair brief; Rady never writes to a Dependabot branch.

[Upgrading](docs/UPGRADING.md) · [Security](docs/SECURITY.md) · [Contributing](docs/CONTRIBUTING.md) · [MIT](LICENSE)

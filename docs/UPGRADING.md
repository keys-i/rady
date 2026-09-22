# Upgrading to the Rust Rady CLI

Rady is now a native Rust executable. Python, virtual environments, `pip`, `uv`, root Python launchers, and the Python package are gone.

Build from a checkout:

```sh
cargo build --release --locked
target/release/rady --help
```

Replace `.venv/bin/rady` with the installed `rady` binary. Replace the legacy setup command with `rady dependasolve`; the `dependasolver` binary remains for compatibility.

## What stays compatible

JSON specifications, evidence semantics, harness environment variables, and GitHub App credentials continue to work. Human output is now the default. Successful `code` and `dependasolve` output keeps the shared `{"schema":1,"status":"ok","kind":"…","result":…}` envelope, so automation should read below `result` and request `--output json` when consuming stdout. Use `--theme dawn|moss|tide|dusk`, or leave `auto` to follow the display profile.

Code runs retain `run.json` plus a self-contained `run.html`; the report renders safe Markdown and LaTeX locally without network requests.

## Solver and harnesses

Setup resolves `keys-i/rady`'s current default-branch commit and pins its immutable 40-character SHA in the reusable workflow. Supply `--solver-ref keys-i/rady@40_CHARACTER_COMMIT_SHA` only to deliberately override it. Other source repositories are rejected before checkout because execution happens on the trusted self-hosted runner.

Rady uses an authenticated Codex or Claude Code CLI, or an operator-owned command adapter. It does not require `OPENAI_API_KEY`. `RADY_MODEL_CHOICES` lists models from least to most capable; `RADY_MODEL` pins one. Broad code work becomes at most eight serial tasks, with escalation only after failed evidence. The public/private runner boundary is unchanged and documented in [Security](SECURITY.md).

Only owners, members, and collaborators can use `@radyybot <prompt>` in an issue or pull request. It returns read-only evidence; bare `@radyybot` shows usage and never edits code.

## Hosted mention replies

On your machine, Rady prefers authenticated Codex. GitHub-hosted replies use a Gemini-first route. Add `RADY_GEMINI_API_KEY` as a repository- or organisation-scoped Actions secret; `RADY_CEREBRAS_API_KEY` and `RADY_XAI_API_KEY` add optional fallbacks. Private and unknown repositories use Gemini or Grok only when `RADY_GEMINI_PRIVATE_OK=true` or `RADY_XAI_PRIVATE_OK=true` opts in. This configuration affects read-only mentions, not native code, Dependasolve, or repair harnesses. [Security](SECURITY.md) records the provider order and data-handling limits.

## GitHub App and dependency reviews

Rady uses one **Rady** App for Dependabot reviews, guarded mentions, and low-risk replacement PRs. Grant Administration, Checks, and Commit statuses read access plus Contents, Issues, and Pull requests write access. Existing installations must accept the added Contents and Issues write permissions. Do not grant branch-protection bypass.

| App | Actions variables | Actions secret |
| --- | --- | --- |
| Rady | `RADY_APP_CLIENT_ID`, `RADY_APP_SLUG` | `RADY_APP_PRIVATE_KEY` |
| Hosted mention opt-in | `RADY_GEMINI_PRIVATE_OK=true`, `RADY_XAI_PRIVATE_OK=true` | `RADY_GEMINI_API_KEY`; optional `RADY_CEREBRAS_API_KEY`, `RADY_XAI_API_KEY` |
| Legacy Dependasolver fallback | `DEPENDASOLVER_APP_CLIENT_ID`, `DEPENDASOLVER_APP_SLUG` | `DEPENDASOLVER_APP_PRIVATE_KEY` |

A complete legacy credential set is used only when every Rady credential is absent; `--app dependasolver` explicitly selects it. Run `rady dependasolve` as a preview, then repeat with `--apply`. It defaults to Rady and reuses a complete verified `RADY_APP_*` set; `--new-app` forces registration. `--check` selects CI evidence to read; `--checks` remains an alias. Setup never changes branch protection.

Rady enables Dependabot auto-merge only when existing protection is strict, administrator-enforced, and requires at least one check. Otherwise it leaves the decision for a maintainer. Scheduled and manual runs visit eligible PRs oldest-first: every non-draft private PR, or trusted public same-repository Dependabot, owner, member, and collaborator PRs. Eligible conflicted Dependabot updates become replacement PRs from the current base; Rady never changes the Dependabot branch.

When upgrading `keys-i/rady`, merge reusable `solve.yml` and `respond.yml` before regenerating its caller workflow. Then run setup from that published revision so `.github/workflows/dependasolver.yml` pins a commit containing both workflows.

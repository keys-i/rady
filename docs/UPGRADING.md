# Upgrading to the Rust Rady CLI

Rady is now a native Rust executable. Python, virtual environments, `pip`, `uv`, the root Python launchers and the Python package are no longer part of the product.

Build from a pinned source revision:

```sh
cargo build --release --locked
target/release/rady --help
```

Replace `.venv/bin/rady` with the installed `rady` binary. Replace the legacy setup command with `rady dependasolve`; a `dependasolver` compatibility binary remains available.

Existing JSON specifications, evidence semantics, harness environment variables and GitHub App credentials remain compatible. Human output is now the default. Successful `code` and `dependasolve` output uses the shared `{"schema":1,"status":"ok","kind":"…","result":…}` envelope, so automation should read command data below `result`. Select `--output json` anywhere a script consumes structured standard output. Select a stable theme with `--theme dawn|moss|tide|dusk`, or keep `auto` to follow the display profile.

Coding runs now retain both `run.json` and a self-contained `run.html`. The HTML report renders safe Markdown and LaTeX locally and makes no network requests.

Setup resolves `keys-i/rady`'s current default-branch commit and pins its immutable 40-character SHA in the reusable workflow. Use `--solver-ref keys-i/rady@40_CHARACTER_COMMIT_SHA` only when explicitly overriding that source. Other source repositories are rejected before checkout because the workflow builds and runs on the trusted self-hosted runner.

Rady continues to use an authenticated Codex or Claude Code CLI, or an operator-owned command adapter. It does not need `OPENAI_API_KEY`. Set `RADY_MODEL_CHOICES` to a comma-separated native-harness model list from least to most capable: `rady code` decomposes broad work into at most eight serial scoped tasks, chooses the least sufficient model per task, and escalates only after failing evidence, while pull-request and Dependasolve reviews choose from their bounded evidence. `RADY_MODEL` pins one model and overrides that choice. Private repositories review every open, non-draft pull request on trusted self-hosted runners. Public repositories admit only same-repository Dependabot updates or same-repository pull requests from an owner, member, or collaborator after GitHub-hosted preflight, and require the bounded Codex or Claude harness. Public forks and custom adapters are rejected. On issues and pull requests, only repository owners, members, and collaborators may invoke `@radyybot <prompt>` for a read-only evidence response; bare `@radyybot` gives concise usage and comment requests never edit code.

Mention replies now run on GitHub-hosted Ubuntu through a local, no-extra-call router. On public repositories, simple questions use free Gemini 3.5 Flash-Lite before Cerebras `gpt-oss-120b`; balanced questions use Cerebras `zai-glm-4.7` then GPT-OSS; deep questions use Gemini 3.8 Flash, then GLM, then GPT-OSS. Add `RADY_GEMINI_API_KEY` and `RADY_CEREBRAS_API_KEY` as Actions secrets at repository or organisation scope. Private and unknown repositories skip both Gemini models unless the optional `RADY_GEMINI_PRIVATE_OK=true` Actions variable explicitly enables them, because Gemini's free tier may use content to improve Google products. Gemini Free quotas vary by Google project and are shown in AI Studio. Cerebras's Free limits are organisation-specific and can change or be reduced temporarily, and the Free tier has no automatic billing. Cerebras says API inputs and outputs are not retained or used for training, although the responder still sends bounded mention and issue or pull-request content to Gemini or Cerebras. A local router is used instead of a separate third-party routing API, so no separate router credential or data recipient is added. This configuration affects only read-only `@radyybot` replies, not native code, Dependasolve, or repair harnesses.

Rady uses one **Rady** App for Dependabot reviews, guarded `@radyybot` responses and low-risk conflict replacement PRs. It needs Administration, Checks and Commit statuses read access plus Contents, Issues and Pull requests write access. Existing installations must accept the added Contents and Issues write permissions before replacement PRs and mention responses can work. Do not grant a branch-protection bypass. A complete legacy Dependasolver credential set is accepted only when all Rady credentials are absent; `--app dependasolver` selects that legacy identity explicitly.

| App | Actions variables | Actions secret |
| --- | --- | --- |
| Rady | `RADY_APP_CLIENT_ID`, `RADY_APP_SLUG` | `RADY_APP_PRIVATE_KEY` |
| Hosted mention replies | `RADY_GEMINI_PRIVATE_OK=true` for private-repository opt-in | `RADY_GEMINI_API_KEY`, `RADY_CEREBRAS_API_KEY` |
| Legacy Dependasolver fallback | `DEPENDASOLVER_APP_CLIENT_ID`, `DEPENDASOLVER_APP_SLUG` | `DEPENDASOLVER_APP_PRIVATE_KEY` |

Run `rady dependasolve` first as a preview, then repeat with `--apply`. It defaults to Rady and reuses a complete `RADY_APP_*` set after owner and permission verification. `--new-app` forces registration for the selected identity. Setup does not change branch protection: each `--check` selects CI evidence for Rady to read, while `--checks` remains a compatibility alias. Rady enables Dependabot auto-merge only where existing protection is strict, administrator-enforced, and requires at least one check; otherwise it leaves a reviewed, held decision for a maintainer. Scheduled and manual reviews take a bounded oldest-first pass through every eligible pull request: all non-drafts in private repositories, or trusted same-repository Dependabot, owner, member, and collaborator pull requests in public repositories. Eligible conflicted Dependabot updates are recreated from the current base as separate replacement PRs; Rady never modifies the Dependabot branch.

When upgrading the `keys-i/rady` repository itself, merge the reusable `solve.yml` and `respond.yml` workflows before regenerating its caller workflow. Then run setup from that published revision so `.github/workflows/dependasolver.yml` pins a commit that already contains both reusable workflows.

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

Rady continues to use an authenticated Codex or Claude Code CLI, or an operator-owned command adapter. It does not need `OPENAI_API_KEY`. Public repositories admit only same-repository Dependabot updates after a GitHub-hosted preflight and require the bounded Codex or Claude harness. Private repositories retain normal pull-request review and operator-owned command adapters on trusted self-hosted runners.

Rady uses one **Rady** App for Dependabot and normal pull-request reviews. It needs Administration, Contents, Checks and Commit statuses read access plus Pull requests write access. Do not grant a branch-protection bypass. A complete legacy Dependasolver credential set is accepted only when all Rady credentials are absent; `--app dependasolver` selects that legacy identity explicitly.

| App | Actions variables | Actions secret |
| --- | --- | --- |
| Rady | `RADY_APP_CLIENT_ID`, `RADY_APP_SLUG` | `RADY_APP_PRIVATE_KEY` |
| Legacy Dependasolver fallback | `DEPENDASOLVER_APP_CLIENT_ID`, `DEPENDASOLVER_APP_SLUG` | `DEPENDASOLVER_APP_PRIVATE_KEY` |

Run `rady dependasolve` first as a preview, then repeat with `--apply`. It defaults to Rady and reuses a complete `RADY_APP_*` set after owner and permission verification. `--new-app` forces registration for the selected identity.

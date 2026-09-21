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

Update the reusable workflow and its `solver-ref` together to the same published 40-character commit from `keys-i/dependasolver`. Other source repositories are rejected before checkout because the workflow builds and runs on the trusted self-hosted runner.

Rady continues to use an authenticated Codex or Claude Code CLI, or an operator-owned command adapter. It does not need `OPENAI_API_KEY`. The review workflow remains limited to private repositories on trusted self-hosted runners.

Keep the existing **Dependasolver** App for Dependabot pull requests and the separate **Rady** App for other reviews. Both need Administration, Contents, Checks and Commit statuses read access plus Pull requests write access. Do not grant a branch-protection bypass.

| App | Actions variables | Actions secret |
| --- | --- | --- |
| Dependasolver | `DEPENDASOLVER_APP_CLIENT_ID`, `DEPENDASOLVER_APP_SLUG` | `DEPENDASOLVER_APP_PRIVATE_KEY` |
| Rady | `RADY_APP_CLIENT_ID`, `RADY_APP_SLUG` | `RADY_APP_PRIVATE_KEY` |

Run `rady dependasolve` first as a preview, then repeat with `--apply`. Existing Apps are reused after their owner and permissions are verified; `--new-app` replaces credentials only for the selected identity.

# Upgrading to Rady 0.6.1

Rady is a native Rust executable. The old Python package, virtual environments, `pip`, `uv`, and root Python launchers are gone.

```sh
cargo build --release --locked
target/release/rady --help
```

Replace `.venv/bin/rady` with `rady`. `dependasolver` remains a compatibility name for `rady dependasolve`.

## What remains compatible

JSON specifications, retained evidence, harness environment variables, and `--output json` remain supported. Human output is now the default. Successful `code` and `dependasolve` commands retain the `{"schema":1,"status":"ok","kind":"…","result":…}` envelope.

`rady code` keeps `run.json` and a self-contained `run.html`; Markdown and LaTeX render locally. Use `--theme dawn|moss|tide|dusk`, or leave `auto` enabled.

## Agent workflows

Rady 0.6.1 adds local skills, opt-in stdio MCP servers, bounded conversation memory, and follow-up questions. Root `AGENTS.md` and `DESIGN.md` are read as pinned guidance for planning, implementation, and review. `.rady/context.json` may list up to 16 skill files and eight MCP servers; no server starts until it is named with `--mcp` on a write run.

Read-only work uses `rady agent ask` and `rady agent follow-up` without creating an edit worktree. Change work remains isolated. Remote pull-request delivery records progressive task checkpoints; pass `--ghost` for one final verified commit. Obvious intent is classified locally, ambiguous intent uses the fast model path, and deep hosted answers chain an evidence brief into the final model.

The central `keys-i/rady` orchestration workflow is the normal service host. It discovers App installations and refreshes their short-lived tokens itself. It polls every five minutes; it is not a webhook service.

An operator who needs a continuously running self-hosted deployment can instead start one persistent service with the App client ID and private-key file:

```sh
rady agent serve --app-client-id CLIENT_ID \
  --app-private-key-file /secure/path/radyybot.pem
```

Use `--owner OWNER` to narrow the self-hosted service or shard a larger installation set.

The scheduled Actions workflow remains a fallback and still supplies Dependabot compatibility metadata used by protected auto-merge. Existing `rady code` and `rady dependasolve` behaviour stays available.

## Central GitHub setup

Rady 0.6.1 uses the public **radyybot** App and one central service. The private key, App client ID and slug, and Gemini, Cerebras, and optional xAI keys stay in the trusted `keys-i/rady` service, never in a target repository.

Install the App, review [Terms](TERMS.md) and [Privacy](PRIVACY.md), then use the guided setup:

```sh
rady setup
```

For automation or a non-interactive shell, use `rady setup --repo keys-i/REPO --check test --accept-terms`. Setup creates a closed, admin-authored consent receipt and writes its IDs, checked CI names, and policy versions to `.github/rady.json`. Review and make that small public configuration commit. Central processing revalidates the receipt and current admin access. Setup creates Dependabot configuration only if none exists and does not alter branch protection. Re-run with `--no-overwrite` when you want setup to refuse an existing generated configuration.

The central workflow covers installed accounts within its bounded installation set; repository consent still gates processing. `rady agent serve` is the self-hosted alternative, not a required target-repository step.

Repository administrators install the App and run `rady setup`; they never configure service credentials. Polling means work begins on the next cycle. Real-time hosting would also need verified GitHub webhooks.

`rady dependasolve` remains available for scripts and its `--solver-ref keys-i/rady@40_CHARACTER_COMMIT_SHA` option deliberately pins an older trusted source. Omit it to use the current `keys-i/rady` commit.

## Mentions and providers

Only an `OWNER`, `MEMBER`, or `COLLABORATOR` may use `@radyybot <prompt>`. It returns a concise, evidence-based answer; it cannot modify code, create a PR, or merge work.

GitHub-hosted mentions prefer compatible Gemini and Cerebras models available to the central credentials. Rady inventories those provider catalogs at runtime and falls back after an unavailable model or quota response. Accessibility in a catalog is not a free-tier guarantee, and Rady never evades quotas or provider terms.

Public repository evidence may use configured providers. Private or unknown repositories need a separate central opt-in for each provider:

| Provider | Central Actions secret | Private-repository variable |
| --- | --- | --- |
| Gemini | `RADY_GEMINI_API_KEY` | `RADY_GEMINI_PRIVATE_OK=true` |
| Cerebras | `RADY_CEREBRAS_API_KEY` | `RADY_CEREBRAS_PRIVATE_OK=true` |
| xAI (optional) | `RADY_XAI_API_KEY` | `RADY_XAI_PRIVATE_OK=true` |

These values belong in `keys-i/rady`, not a target repository. See [Security](SECURITY.md) before enabling a provider for private content.

## Dependency reviews

Rady reads selected CI evidence; `--check` (and its `--checks` alias) does not run a shell command or make GitHub require a check. Eligible work is processed oldest first. The scheduled workflow can enable protected auto-merge for a clean, verified update after checking Dependabot metadata. The persistent service posts reviews. Conflicted updates remain for local `rady code` repair so long-lived central keys never enter a self-hosted runner.

Native code and repair runs use an authenticated Codex or Claude Code CLI, or an operator-owned adapter. `RADY_MODEL_CHOICES` orders native models from least to most capable and `RADY_MODEL` pins one. Hosted providers answer mentions and perform central read-only review; they do not replace the local code or repair harness.

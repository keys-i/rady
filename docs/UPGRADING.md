# Upgrading to Rady 0.6.6

Rady is a native Rust executable. The old Python package, virtual environments, `pip`, `uv`, and root Python launchers are gone.

```sh
cargo build --release --locked
target/release/rady --help
```

Replace `.venv/bin/rady` with `rady`. `dependasolver` remains a compatibility name for `rady dependasolve`.

## 0.6.6

This compatible release adds opt-in browser verification for frontend work. Configure a local MCP named `browser` in `.rady/context.json`, then select it with `rady code "Check http://127.0.0.1:3000" --browser`. The browser stays local and starts only for that write run.

It also adds free-first routing across six providers with bounded catalogs, retries, cooldowns, and an evidence scout for Deep work. Optional authenticated-loopback Laya can classify intent and tier selection; temporary hosted-model outages remain retryable so pending work is preserved. Provider credential isolation and Privacy-policy consent are hardened.

## 0.6.3

This is a compatible patch release. Setup now uses one readable seven-stage progress line, App authentication keeps JWTs out of process arguments, and new App manifests request read-only repository contents access. Existing App installations keep their current permissions; change **Contents** to **Read-only** in the App settings to adopt the narrower permission.

## What remains compatible

JSON specifications, retained evidence, harness environment variables, and `--output json` remain supported. Human output is now the default. Successful `code` and `dependasolve` commands retain the `{"schema":1,"status":"ok","kind":"…","result":…}` envelope.

`rady code` keeps `run.json` and a self-contained `run.html`; Markdown and LaTeX render locally. Use `--theme dawn|moss|tide|dusk`, or leave `auto` enabled.

## Agent workflows

Rady 0.6.0 added local skills, opt-in stdio MCP servers, bounded conversation memory, and follow-up questions. Root `AGENTS.md` and `DESIGN.md` are read as pinned guidance for planning, implementation, and review. `.rady/context.json` may list up to 16 skill files and eight MCP servers; no server starts until it is named with `--mcp` on a write run.

Read-only work uses `rady agent ask` and `rady agent follow-up` without creating an edit worktree. Change work remains isolated. Remote pull-request delivery records progressive task checkpoints; pass `--ghost` for one final verified commit. Obvious intent is classified locally, ambiguous intent uses the fast model path, and deep hosted answers chain an evidence brief into the final model.

For a self-hosted decision classifier, run Laya locally with `LAYA_HOST=127.0.0.1`, a non-empty `LAYA_API_KEY`, and `LAYA_PRELOAD=1`; set `RADY_LAYA_ENABLED=true` and the matching `RADY_LAYA_API_KEY`. Rady calls only `http://127.0.0.1:8000/v1/systemone`. Laya selects intent and a model tier only, cannot draft a response, and may only preserve or escalate the tier selected by deterministic rules. The existing hosted classifier remains the fallback. Laya needs operator-provided local compute and model storage (about 647–808 MB in its published guidance), so the GitHub-hosted service never enables it.

Rady 0.6.1 added the persistent App-authenticated service. The central `keys-i/rady` orchestration workflow discovers App installations and refreshes their short-lived tokens itself. It polls every five minutes; it is not a webhook service.

An operator who needs a continuously running self-hosted deployment can instead start one persistent service with the App client ID and private-key file:

```sh
rady agent serve --app-client-id CLIENT_ID \
  --app-private-key-file /secure/path/radyybot.pem
```

Use `--owner OWNER` to narrow the self-hosted service or shard a larger installation set.

The scheduled Actions workflow remains a fallback and still supplies Dependabot compatibility metadata used by protected auto-merge. Existing `rady code` and `rady dependasolve` behaviour stays available.

## Central GitHub setup

Rady 0.6.2 adds one guided setup for the public **radyybot** App and central service. The private key, App client ID and slug, and configured model credentials stay in the trusted `keys-i/rady` service, never in a target repository.

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

GitHub-hosted mentions use compatible centrally configured models and fall back after an unavailable model or quota response. Groq and Cloudflare Workers AI have recurring free allocations under their provider terms; OpenRouter is a low-quota opt-in fallback. Catalog access is not an unlimited or guaranteed free tier, and Rady never rotates keys or accounts to evade quotas or provider terms.

Public repository evidence may use configured providers. Private or unknown repositories need a separate central opt-in for each provider:

| Provider | Central service configuration | Private-repository variable |
| --- | --- | --- |
| Gemini | `RADY_GEMINI_API_KEY` | `RADY_GEMINI_PRIVATE_OK=true` |
| Cerebras | `RADY_CEREBRAS_API_KEY` | `RADY_CEREBRAS_PRIVATE_OK=true` |
| xAI (optional) | `RADY_XAI_API_KEY` | `RADY_XAI_PRIVATE_OK=true` |
| Groq | `RADY_GROQ_API_KEY` | `RADY_GROQ_PRIVATE_OK=true` |
| Cloudflare Workers AI | `RADY_CLOUDFLARE_API_TOKEN` and `RADY_CLOUDFLARE_ACCOUNT_ID` | `RADY_CLOUDFLARE_PRIVATE_OK=true` |
| OpenRouter (low-quota fallback) | `RADY_OPENROUTER_API_KEY` | `RADY_OPENROUTER_PRIVATE_OK=true` |

These values belong in `keys-i/rady`, not a target repository. See [Security](SECURITY.md) before enabling a provider for private content.

The added processors advance the Privacy policy to `2026-09-25`. Re-run `rady setup` as a repository administrator to review and record fresh consent; central processing stays paused until then.

## Dependency reviews

Rady reads selected CI evidence; `--check` (and its `--checks` alias) does not run a shell command or make GitHub require a check. Eligible work is processed oldest first. The scheduled workflow can enable protected auto-merge for a clean, verified update after checking Dependabot metadata. The persistent service posts reviews. Conflicted updates remain for local `rady code` repair so long-lived central keys never enter a self-hosted runner.

Native code and repair runs use an authenticated Codex or Claude Code CLI, or an operator-owned adapter. `RADY_MODEL_CHOICES` orders native models from least to most capable and `RADY_MODEL` pins one. Hosted providers answer mentions and perform central read-only review; they do not replace the local code or repair harness.

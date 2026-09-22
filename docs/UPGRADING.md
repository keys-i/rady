# Upgrading to Rady 0.5.6

Rady is a native Rust executable. The old Python package, virtual environments, `pip`, `uv`, and root Python launchers are gone.

```sh
cargo build --release --locked
target/release/rady --help
```

Replace `.venv/bin/rady` with `rady`. `dependasolver` remains a compatibility name for `rady dependasolve`.

## What remains compatible

JSON specifications, retained evidence, harness environment variables, and `--output json` remain supported. Human output is now the default. Successful `code` and `dependasolve` commands retain the `{"schema":1,"status":"ok","kind":"…","result":…}` envelope.

`rady code` keeps `run.json` and a self-contained `run.html`; Markdown and LaTeX render locally. Use `--theme dawn|moss|tide|dusk`, or leave `auto` enabled.

## Central GitHub setup

Rady 0.5.6 uses the public **radyybot** App and one central Actions installation in `keys-i/rady`. The private key, App client ID and slug, and Gemini, Cerebras, and optional xAI keys live there only. They must not be added to target repositories.

For a `keys-i` repository, install the App, review [Terms](TERMS.md) and [Privacy](PRIVACY.md), then run:

```sh
rady dependasolve --repo keys-i/REPO --check test
rady dependasolve --repo keys-i/REPO --check test --apply --accept-terms
```

Setup creates a closed, admin-authored consent receipt and records its IDs, the checked CI names, and policy versions in `.github/rady.json`. Central processing revalidates the receipt and current admin access. Setup creates Dependabot configuration only if none exists and does not alter branch protection. Re-run with `--no-overwrite` when you want setup to refuse an existing generated configuration.

Central Actions checks consented installed repositories every five minutes. That is the secure GitHub-only arrangement for `keys-i` repositories, not instant delivery. A repository owned by someone else, or a real-time service, needs a hosted backend with a dedicated secret manager and verified GitHub webhooks; a reusable workflow cannot read secrets from `keys-i/rady` on behalf of another repository.

`--solver-ref keys-i/rady@40_CHARACTER_COMMIT_SHA` is only for deliberately pinning an older trusted source. Omit it to use the current `keys-i/rady` commit.

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

Rady reads selected CI evidence; `--check` (and its `--checks` alias) does not run a shell command or make GitHub require a check. Eligible work is processed oldest first. The central service can enable protected auto-merge for a clean, verified update. Conflicted updates remain for local `rady code` repair so long-lived central keys never enter a self-hosted runner.

Native code and repair runs use an authenticated Codex or Claude Code CLI, or an operator-owned adapter. `RADY_MODEL_CHOICES` orders native models from least to most capable and `RADY_MODEL` pins one. Hosted providers answer mentions and perform central read-only review; they do not replace the local code or repair harness.

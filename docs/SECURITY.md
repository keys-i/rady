# Security policy

## Report privately

Use this repository's **Security** tab → **Report a vulnerability**. Do not open a public issue. Include a minimal reproduction, affected version or commit, impact, and a safe mitigation. Remove credentials, tokens, personal data, and production content.

`main` is the supported line. Fixes land there; backports and response times are not promised.

## Central credential boundary

The scheduled service in `keys-i/rady` keeps the App private key and model keys in that repository’s GitHub Actions secrets. Its App client ID and slug are central variables. Target repositories receive only public `.github/rady.json` configuration, never a private key, client ID, model key, token, or reusable-workflow secret.

The central workflow runs `rady agent serve --once`, discovers up to 256 App installations, and processes only repositories with a valid consent receipt. A self-hosted service may shard a larger deployment with `--owner`. Rady mints bounded, least-privilege installation tokens itself. This is polling, not a public webhook endpoint: expect work to begin on the next cycle. Real-time hosting still requires HTTPS webhook verification and delivery idempotency; do not copy central secrets into target repositories to simulate it.

## Consent and data handling

Before central processing begins, an authorised repository administrator must install radyybot and run `rady setup`. For automation, use `rady setup --repo owner/repo --check test --accept-terms`. Rady creates a closed, admin-authored GitHub consent receipt and stores its IDs, accepting login, time, and current policy versions in the small public `.github/rady.json` configuration. Review and commit it. Central processing revalidates the exact receipt and the signer’s current admin access. A policy-version change pauses processing until it is accepted again. Read the [Terms](TERMS.md) and [Privacy policy](PRIVACY.md).

Only `OWNER`, `MEMBER`, and `COLLABORATOR` actors can invoke `@radyybot <prompt>`. A mention produces read-only evidence; it cannot write code, open a pull request, or merge. Comments, pull-request text, checks, diffs, paths, and model output are untrusted input.

## Hosted models

GitHub-hosted mentions use compatible configured models from Gemini, Cerebras, xAI, Groq, Cloudflare Workers AI, and OpenRouter. Groq and Cloudflare Workers AI have recurring free allocations subject to their provider terms; OpenRouter is an explicit low-quota fallback. Model catalogs show accessibility, not price, capacity, or an unlimited entitlement. Rady handles unavailable models and quota responses by trying the next permitted provider; it does not rotate keys or accounts, evade limits, or otherwise bypass provider controls.

Cooldowns and rejected-model memory last for the service process. The scheduled Actions fallback starts cold on each run, so remove a credential that keeps returning an entitlement error; the persistent service retains health state between cycles.

An operator may opt into a local Laya classifier with `RADY_LAYA_ENABLED=true`. Run Laya with `LAYA_HOST=127.0.0.1` and a non-empty `LAYA_API_KEY`, then set `RADY_LAYA_API_KEY` to the same value. Rady connects only to `http://127.0.0.1:8000/v1/systemone`; the key is removed from native-agent children and must not be placed in target repositories. Laya receives only the bounded decision input needed to classify intent and model tier. It cannot generate replies, lower a selected tier, or replace deterministic and hosted-classifier fallbacks. GitHub-hosted workflows never enable it because a hosted runner cannot safely reach an operator's loopback service.

Provider keys remain central and are removed from native-agent children. Rady sends a bounded copy of relevant issue or pull-request evidence only to the selected provider. Public repository evidence may use configured providers. Private and unknown repository evidence is blocked unless the corresponding central variable is exactly `true`:

| Provider | Opt-in variable |
| --- | --- |
| Gemini | `RADY_GEMINI_PRIVATE_OK` |
| Cerebras | `RADY_CEREBRAS_PRIVATE_OK` |
| xAI | `RADY_XAI_PRIVATE_OK` |
| Groq | `RADY_GROQ_PRIVATE_OK` |
| Cloudflare Workers AI | `RADY_CLOUDFLARE_PRIVATE_OK` |
| OpenRouter | `RADY_OPENROUTER_PRIVATE_OK` |

Do not opt in when you are not allowed to disclose that repository content. Provider retention, training, and regional processing are governed by the provider account and terms in effect for that request.

## Execution and delivery

Use authenticated agent CLIs only on machines and runners you control. `RADY_MODEL_CHOICES` orders native-harness models and `RADY_MODEL` pins one. Never put subscriptions, API keys, or interactive harness logins in a public workflow.

Rady blocks known credential and private-key files, works in isolated worktrees, bounds subprocess time and output, and checks protected files before publication. It fails closed when evidence is absent. App tokens are short-lived and are given only to authenticated Git and GitHub API children.

The central service performs read-only reviews and leaves merging under repository protection. A self-hosted deployment uses `--app-client-id` and `--app-private-key-file`; Rady reads the bounded key only to refresh tokens and never writes those tokens to disk. `--token-command` remains advanced compatibility for a dedicated secret broker that prints one fresh installation token to stdout per cycle. Conflicted updates remain for an operator to repair with local `rady code`; local credentials and retained evidence stay under that operator's control.

Repository `AGENTS.md`, `DESIGN.md`, configured skills, issue comments, and MCP definitions are untrusted input. Guidance must be regular, non-symlink UTF-8 files under the repository root and is pinned for the run. Skills are text only. MCP is disabled by default, supports selected local stdio commands only, and runs with credential-shaped environment variables removed. Selecting an MCP server authorises that local program; it is privileged local code, so inspect it first.

`rady code --browser` is an explicit write-run opt-in for a locally preinstalled, reviewed, version-pinned Microsoft Playwright MCP command named `playwright-mcp`. The reserved `browser` entry accepts only the exact isolated, sandboxed, no-WebMCP, no-service-worker, bounded argument set shown in the README. Rady never downloads it and enables it only when selected with `--browser` or the equivalent `--mcp browser`. It is still privileged local code, not a sandbox: use only an explicit local or user-supplied preview URL, do not log in, use production credentials, upload, download, grant permissions, or make production mutations. Read-only agent questions and the hosted service never receive browser MCP access.

Conversation memory is private, atomic, bounded, and stores only the visible questions and answers. It does not store provider keys or hidden reasoning. Progressive commits exist only inside Rady's isolated delivery branch until every gate passes; `--ghost` changes commit shape, not verification.

`RADY_AGENT_COMMAND` and `RADY_REVIEW_COMMAND` are privileged local programs, not sandboxes. Review them and give them the least access possible. Local HTML reports escape raw HTML, neutralise unsafe links, and use no scripts, remote fonts, or network resources. Retained evidence can contain paths, diffs, and command output; protect it accordingly.

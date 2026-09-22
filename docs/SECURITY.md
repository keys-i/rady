# Security policy

## Report privately

Use this repository's **Security** tab → **Report a vulnerability**. Do not open a public issue. Include a minimal reproduction, affected version or commit, impact, and a safe mitigation. Remove credentials, tokens, personal data, and production content.

`main` is the supported line. Fixes land there; backports and response times are not promised.

## Central credential boundary

For the hosted `keys-i` service, App credentials and model keys are GitHub Actions secrets in `keys-i/rady` only. Target repositories receive `.github/rady.json`, never a private key, client ID, model key, or reusable-workflow secret. The central workflow mints short-lived, repository-scoped App tokens.

This arrangement covers App-installed, consented repositories owned by `keys-i` and runs on a five-minute schedule. It is not a general cross-owner or real-time service. Those cases require a hosted backend, a separate secret store, HTTPS webhook verification, and delivery idempotency; do not copy central secrets into another repository to simulate it.

## Consent and data handling

Before central processing begins, an authorised repository administrator must install radyybot and run `rady dependasolve --apply --accept-terms`. Rady creates a closed, admin-authored GitHub consent receipt and stores its IDs, the accepting login, time, and current policy versions in `.github/rady.json`. Central processing revalidates the exact receipt and the signer’s current admin access. A policy-version change pauses processing until it is accepted again. Read the [Terms](TERMS.md) and [Privacy policy](PRIVACY.md).

Only `OWNER`, `MEMBER`, and `COLLABORATOR` actors can invoke `@radyybot <prompt>`. A mention produces read-only evidence; it cannot write code, open a pull request, or merge. Comments, pull-request text, checks, diffs, paths, and model output are untrusted input.

## Hosted models

GitHub-hosted mentions use compatible Gemini and Cerebras models visible to the configured accounts, with xAI as an optional fallback. Model catalogs show accessibility, not price, capacity, or a free-tier entitlement. Rady handles unavailable models and quota responses by trying the next permitted model; it does not rotate accounts, evade limits, or otherwise bypass provider controls.

Provider keys remain central and are removed from native-agent children. Rady sends a bounded copy of relevant issue or pull-request evidence only to the selected provider. Public repository evidence may use configured providers. Private and unknown repository evidence is blocked unless the corresponding central variable is exactly `true`:

| Provider | Opt-in variable |
| --- | --- |
| Gemini | `RADY_GEMINI_PRIVATE_OK` |
| Cerebras | `RADY_CEREBRAS_PRIVATE_OK` |
| xAI | `RADY_XAI_PRIVATE_OK` |

Do not opt in when you are not allowed to disclose that repository content. Provider retention, training, and regional processing are governed by the provider account and terms in effect for that request.

## Execution and delivery

Use authenticated agent CLIs only on machines and runners you control. `RADY_MODEL_CHOICES` orders native-harness models and `RADY_MODEL` pins one. Never put subscriptions, API keys, or interactive harness logins in a public workflow.

Rady blocks known credential and private-key files, works in isolated worktrees, bounds subprocess time and output, and checks protected files before publication. It fails closed when evidence is absent. App tokens are short-lived and are given only to authenticated Git and GitHub API children.

The central service performs read-only review and protected auto-merge on GitHub-hosted runners. It does not expose the App private key or model keys to a persistent self-hosted runner. Conflicted updates remain for an operator to repair with local `rady code`; local credentials and retained evidence stay under that operator's control.

`RADY_AGENT_COMMAND` and `RADY_REVIEW_COMMAND` are privileged local programs, not sandboxes. Review them and give them the least access possible. Local HTML reports escape raw HTML, neutralise unsafe links, and use no scripts, remote fonts, or network resources. Retained evidence can contain paths, diffs, and command output; protect it accordingly.

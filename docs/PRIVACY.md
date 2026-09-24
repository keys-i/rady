# Rady privacy policy

Effective 25 September 2026 · version `2026-09-25`

This policy describes the hosted RadDuck service maintained through `keys-i/rady`. A self-hosted Rady operator is responsible for that deployment's privacy practices.

## What Rady handles

Rady receives the GitHub account and repository information made available to its installation. That can include usernames, author associations, issue and pull-request text, comments, diffs, filenames, commit identifiers, check results and links. It also creates a closed consent issue and records its IDs, the accepting GitHub login, policy versions and acceptance time in `.github/rady.json`.

Rady uses this information to authenticate requests, answer `@radduck` mentions, review eligible pull requests, prevent duplicate work, diagnose failures and protect the service. It does not sell personal information or use repository content to advertise to you.

## Where information goes

GitHub hosts the App installation, repository data, comments, reviews, configuration and Actions logs. Hosted model requests may send a bounded copy of relevant evidence to Google Gemini, Cerebras, xAI, Groq, Cloudflare Workers AI, or OpenRouter when centrally configured. Public repositories may use configured providers by default. Private or unknown repositories require a separate per-provider opt-in. Each provider handles submitted data under its own terms and privacy controls, and may process it in the United States or other countries listed in its policy.

A self-hosted operator may enable Laya for intent and model-tier decisions. Rady sends its bounded input only to an authenticated fixed loopback endpoint on the same machine; Laya does not generate the answer or receive provider credentials.

App private keys and model credentials stay in the `keys-i/rady` GitHub Actions secret store. Rady removes them from native agent children and does not write them into target repositories or model prompts.

## Retention and control

Rady has no separate user-profile or prompt database. Acceptance records remain in the target repository. GitHub comments, reviews, configuration and Actions logs remain under GitHub and repository retention controls. Model providers retain requests according to the account and provider policy in force when a request is made. Local CLI evidence remains on the machine where Rady ran until its operator removes it.

You can inspect or correct repository-held information through GitHub. You can stop new processing by uninstalling the App, remove `.github/rady.json`, delete comments where GitHub permits, or disable a model provider. For an access, correction or privacy complaint that cannot be handled in the repository, contact the maintainer through the private reporting route in [Security](SECURITY.md) and do not include unnecessary repository content.

## Automated decisions and security

Rady uses rules plus model output to recommend or approve pull-request actions. It exposes evidence and blockers, and it does not bypass branch protection. Repository owners control installation, checks, provider opt-ins and final merge policy.

Rady limits evidence and output sizes, validates trusted actors, uses short-lived repository-scoped App tokens and rejects unaccepted installations. No service can promise absolute security; report suspected exposure promptly so access can be revoked and credentials rotated.

Material changes get a new policy version and pause central processing until an authorised user accepts it.

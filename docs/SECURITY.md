# Security policy

## Report privately

Use this repository’s **Security** tab → **Report a vulnerability**. Do not open a public issue. Include a minimal reproduction, affected version, impact, and safe mitigation. Remove credentials, tokens, personal data, and production content.

`main` is the supported line. Fixes land there; backports and response times are not promised.

## Central boundary

The service host keeps App and model credentials in `keys-i/rady`. Target repositories receive only public `.github/rady.json`; they never receive a private key, client ID, model key, token, or reusable-workflow secret.

The service polls a bounded recent window across consented installations. Mention and discovery operations use short-lived, installation-scoped tokens with only their needed permissions. A selected dependency review receives a repository-scoped token. This is polling, not a webhook endpoint, so work starts on a later cycle.

## Consent

Before central processing, an authorised repository administrator installs RadDuck and runs:

```sh
rady setup --repo owner/repo --check test --accept-terms
```

Setup creates a closed, admin-authored receipt and stores its IDs, signer, time, and policy versions in `.github/rady.json`. The service revalidates the exact receipt and current admin access. A policy-version change pauses processing until setup is run again. Review the [Terms](TERMS.md) and [Privacy policy](PRIVACY.md) first.

## Reviews and mentions

Only `OWNER`, `MEMBER`, and `COLLABORATOR` actors can invoke `@radduck <prompt>`. Replies are read-only. Issue text, pull-request text, checks, diffs, paths, and model output are untrusted input.

Review windows rotate fairly. Rady honours the configured source pin, verifies Dependabot evidence and selected checks, and leaves unsupported, grouped, or ambiguous updates as comments for manual review. The App registration reserves Contents write access for approved coding delivery. Mention, discovery, and review tokens are narrowed to read-only contents access and cannot push or merge.

## Model evidence

Provider credentials remain central. Rady sends only bounded, relevant evidence to the selected configured provider. Private and unknown repository evidence is blocked unless the central operator enables that provider for private content. Do not opt in unless you may disclose the content under that provider’s terms.

Rady bounds evidence and output, rejects unaccepted installations, and fails closed when required evidence is missing. No service can promise absolute security; report suspected exposure promptly so access can be revoked and credentials rotated.

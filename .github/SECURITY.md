# Security policy

## Report privately

Use this repository’s **Security** tab → **Report a vulnerability**. Do not open a public issue. Include a minimal reproduction, affected version, impact, and safe mitigation. Remove credentials, tokens, personal data, and production content.

`main` is the supported line. Fixes land there; backports and response times are not promised.

## Central boundary

The central host keeps App credentials and provider keys in `keys-i/rady`. Target repositories receive only public `.github/koelu.json`; they never receive a private key, client ID, model key, token, or reusable-workflow secret.

The service polls a bounded recent window across consented installations. Mention and discovery operations use short-lived, installation-scoped tokens with only their needed permissions. A selected dependency review receives a repository-scoped token. An approved delivery receives a fresh, short-lived token restricted to its one repository. This is polling, not a webhook endpoint, so work starts on a later cycle.

## Consent

Before central processing, an authorised repository administrator installs Koelu and runs:

```sh
koelu setup --repo owner/repo --check test --accept-terms
```

Setup creates a closed, admin-authored receipt and stores its IDs, signer, time, and policy versions in `.github/koelu.json`. The service revalidates the exact receipt and current admin access. A policy-version change pauses processing until setup is run again. Koelu 0.6.9 requires Terms `2026-09-27-t4` and Privacy `2026-09-27-p4`. Review the [Terms](../docs/TERMS.md) and [Privacy policy](../docs/PRIVACY.md) first.

## Reviews and mentions

Only `OWNER`, `MEMBER`, and `COLLABORATOR` actors can invoke `@koelu <prompt>`. Replies are read-only. Issue text, pull-request text, checks, diffs, paths, and model output are untrusted input.

Review windows rotate fairly. Koelu honours the configured source pin, verifies Dependabot evidence and selected checks, and leaves unsupported, grouped, or ambiguous updates as comments for manual review. The App registration reserves Contents write access for approved coding delivery. Mention, discovery, and review tokens are narrowed to read-only contents access and cannot push or merge.

## Approved writes

A write starts only with an explicit `@koelu` request. Koelu records an exact proposal including the request, author, default branch, and base commit. The same author must approve that request by comment ID. Immediately before dispatch, the service rechecks both comments, that the issue or pull request is open, the proposal, current author permission, and the recorded base revision.

Only then does it mint a repository-scoped delivery token, create an isolated branch, and open a pull request. It never merges, changes branch protection, or writes to the default branch. A claim marker prevents a second dispatch for the same approval. Every claim receives a terminal result; invalid approvals, permission or base changes, cancellation, and failed validation stop the run. Resolve the cause and create a new request and approval rather than reusing a claimed approval.

## Model evidence

App credentials remain central and never enter child processes. Mention and review requests send bounded, relevant evidence to the selected configured provider. An approved hosted edit runs a constrained provider CLI that may read and send repository files it selects for that approved task. Its selected provider API key is passed only to that scrubbed client child, never to the target repository, prompt, or logs. Private and unknown repository evidence is blocked unless the central operator enables that provider for private content. Do not opt in unless you may disclose the content under that provider’s terms.

Koelu bounds evidence and output, rejects unaccepted installations, and fails closed when required evidence is missing. No service can promise absolute security; report suspected exposure promptly so access can be revoked and credentials rotated.

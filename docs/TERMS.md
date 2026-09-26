# Koelu service terms

Effective 27 September 2026 · version `2026-09-27-t4`

These are the current hosted-service terms for Koelu 0.6.9, not a retroactive agreement for earlier Rady or Surkab releases.

These terms cover the hosted Koelu service maintained through `keys-i/rady`. The MIT licence covers the Koelu source code; a self-hosted copy is not this hosted service.

## Your agreement

You must be authorised to connect the selected GitHub account, organisation, and repositories. By passing `--accept-terms`, you accept these terms and the matching [Privacy policy](PRIVACY.md). Koelu records that acceptance in a closed GitHub issue and the repository’s public `.github/koelu.json` file.

## What Koelu does

Koelu reads available repository metadata, issues, pull requests, diffs, and check results to answer `@koelu` and review eligible dependency updates. It may post a comment or review. For an explicit write request, it can prepare an isolated branch and pull request only after the same author approves that exact request. It does not enable, disable, or perform merges. You decide whether to merge.

Before an approved write, Koelu rechecks the request, approval, current author access, recorded proposal, and recorded base revision. It uses a short-lived token scoped to that repository. If a check fails, the base advances, work is cancelled, or validation fails, it stops without changing the default branch and posts a terminal result. Each approval is claimed once to prevent duplicate delivery; make a fresh request and approval after resolving a failed dispatch.

The hosted service polls a bounded recent window and is not real time. Unsupported, grouped, or ambiguous dependency updates are left for manual review. Mention and review requests may send bounded evidence to a centrally configured model provider. An approved hosted edit runs a constrained provider CLI that may read and send repository files it selects for that approved task, as described in the Privacy policy.

## Your responsibilities

Use Koelu lawfully and only where you have authority. Do not use it to expose secrets or personal data, attack a service, evade provider limits, mislead contributors, or disclose content to a provider without permission. Keep branch protection and App access appropriate for your project.

## Availability and changes

Koelu is provided as available and can be incomplete or wrong. Check material advice and changes yourself. Access may be limited or suspended to protect repositories, people, providers, or the service. You can stop processing by uninstalling the App and removing the repository configuration.

Material changes receive a new version. Re-run `koelu setup` as an administrator to accept them before central processing resumes. For security reports, use [Security](../.github/SECURITY.md); do not include confidential repository content in public requests.

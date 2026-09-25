# Rady service terms

Effective 25 September 2026 · version `2026-09-25-t1`

These terms cover the hosted RadDuck service maintained through `keys-i/rady`. The MIT licence covers the Rady source code; a self-hosted copy is not this hosted service.

## Your agreement

You must be authorised to connect the selected GitHub account, organisation, and repositories. By passing `--accept-terms`, you accept these terms and the matching [Privacy policy](PRIVACY.md). Rady records that acceptance in a closed GitHub issue and the repository’s public `.github/rady.json` file.

## What Rady does

Rady reads available repository metadata, issues, pull requests, diffs, and check results to answer `@radduck` and review eligible dependency updates. It may post a comment or review. It does not enable, disable, or perform merges, and it cannot push repository contents. You decide whether to merge.

The hosted service polls a bounded recent window and is not real time. Unsupported, grouped, or ambiguous dependency updates are left for manual review. Rady may send bounded evidence to a centrally configured model provider as described in the Privacy policy.

## Your responsibilities

Use Rady lawfully and only where you have authority. Do not use it to expose secrets or personal data, attack a service, evade provider limits, mislead contributors, or disclose content to a provider without permission. Keep branch protection and App access appropriate for your project.

## Availability and changes

Rady is provided as available and can be incomplete or wrong. Check material advice and changes yourself. Access may be limited or suspended to protect repositories, people, providers, or the service. You can stop processing by uninstalling the App and removing the repository configuration.

Material changes receive a new version. Re-run `rady setup` as an administrator to accept them before central processing resumes. For security reports, use [Security](SECURITY.md); do not include confidential repository content in public requests.

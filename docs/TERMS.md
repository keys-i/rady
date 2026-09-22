# Rady service terms

Effective 23 September 2026 · version `2026-09-23`

These terms cover the hosted radyybot service maintained through `keys-i/rady`. The MIT licence continues to cover the Rady source code. Installing or running your own copy does not turn that copy into a hosted service from keys-i.

## Your agreement

You must be old enough to enter a contract and authorised to connect the selected GitHub account, organisation and repositories. When you pass `--accept-terms`, you confirm that you have read and accept these terms and the matching [Privacy policy](PRIVACY.md). Rady creates a closed GitHub issue with your acceptance comment and records its IDs, your GitHub login, the policy versions and acceptance time in `.github/rady.json`.

## What Rady does

Rady reads repository metadata, issues, pull requests, diffs and check results to answer requested questions and review eligible changes. Depending on your configuration, it may post comments or reviews, approve low-risk dependency changes, or enable protected auto-merge. GitHub remains the source of truth. You remain responsible for reviewing and merging changes.

Rady may send bounded repository evidence to the model providers described in the Privacy policy. Do not enable a provider for content you are not allowed to disclose to it.

## Your responsibilities

Use Rady lawfully. Do not use it to expose secrets or personal information, attack a service, evade provider limits, mislead contributors, or process a repository without authority. Keep branch protection, review rules and App access appropriate for your project. Revoke the App and rotate affected credentials if access may have been compromised.

You give Rady permission to process repository content only to provide and secure the service. You keep ownership of that content.

## Availability and risk

Rady is provided as available and may change, pause or stop. Automated output can be incomplete or wrong. Verify material changes and advice before relying on them. Nothing in these terms excludes rights or guarantees that cannot lawfully be excluded. To the extent the law permits, keys-i is not responsible for indirect or consequential loss arising from use of the service.

Access may be limited or suspended to protect repositories, users, providers or the service. You can stop using Rady at any time by uninstalling the App and removing its configuration.

## Changes and contact

A material terms or privacy change gets a new version and requires fresh acceptance before central orchestration resumes. For security reports, use the private vulnerability-reporting link in [Security](SECURITY.md). For other questions, open a repository discussion or issue without including confidential data.

# Koelu privacy policy

Effective 27 September 2026 · version `2026-09-27-p4`

This is the current hosted-service policy for Koelu 0.6.9, not a retroactive description of earlier Rady or Surkab releases.

This policy covers the hosted Koelu service maintained through `keys-i/rady`. A self-hosted operator is responsible for its own deployment.

## Information Koelu handles

Koelu receives the GitHub account and repository information exposed to its App installation. That can include usernames, issue and pull-request text, comments, diffs, paths, commit IDs, check results, and links. It stores consent receipt IDs, the accepting login, policy versions, and acceptance time in the target repository’s `.github/koelu.json`.

Koelu uses this information to authenticate work, answer `@koelu`, review eligible pull requests, prepare an approved write, prevent duplicate work, and protect the service. It does not sell repository content or use it for advertising.

## Where information goes

GitHub hosts App installations, repository data, comments, reviews, configuration, and Actions logs. Mention and review requests send bounded, relevant evidence to a centrally configured provider. An approved hosted edit runs a constrained provider CLI that may read and send the repository files it selects to complete that approved task. Public-repository evidence may use a configured provider; private or unknown repository evidence needs that provider’s separate opt-in. Provider retention, training, and regional processing follow the provider account and terms in force for that request.

App credentials stay in the central service and out of child processes. The selected provider API key is passed only to its scrubbed provider-client child; it is never written to the target repository, prompt, or logs. The service uses short-lived installation-scoped tokens for mentions and discovery, repository-scoped tokens for selected reviews, and a fresh repository-scoped token only after a write request receives the same author's approval. The delivery token is used to create an isolated branch and pull request, never to merge.

## Retention and control

Koelu has no separate user-profile or prompt database. GitHub comments, reviews, configuration, and Actions logs follow GitHub and repository retention settings. Providers retain requests under their own terms. Local CLI evidence remains on the operator’s machine until removed.

You can stop new processing by uninstalling the App or removing `.github/koelu.json`, and can remove GitHub content where GitHub allows. For an access, correction, or privacy concern that cannot be handled in the repository, use the private route in [Security](../.github/SECURITY.md) without sending unnecessary repository content.

## Automated work

Koelu uses rules and model output to prepare answers and reviews. For a requested code change, it rechecks the exact request, same-author approval, live permission, proposal, and base revision before preparing a branch and pull request. It leaves unsupported or ambiguous dependency updates for manual review and does not enable, disable, or perform merges. Repository owners retain the final merge decision.

Material policy changes get a new version and pause central processing until an authorised administrator reruns setup and accepts them. This version requires a new acceptance for the hosted editing disclosure.

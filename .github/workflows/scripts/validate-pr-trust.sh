#!/usr/bin/env bash
set -euo pipefail

echo 'allowed=false' >> "$GITHUB_OUTPUT"
[[ "$PR_NUMBER" =~ ^[1-9][0-9]*$ ]] || {
  echo 'PR number must be positive' >&2
  exit 1
}
pull=$(gh api "repos/$GH_REPO/pulls/$PR_NUMBER")
jq -e --arg repo "$GH_REPO" --argjson number "$PR_NUMBER" '
  .number == $number and
  .state == "open" and .draft == false and
  .base.repo.full_name == $repo and
  (.head.sha | type == "string" and test("^[a-f0-9]{40}$"))
' <<<"$pull" > /dev/null
dependency=$(jq -r '.user.login == "dependabot[bot]"' <<<"$pull")
if [[ "$PRIVATE_REPOSITORY" != true ]]; then
  jq -e --arg repo "$GH_REPO" '
    .head.repo.full_name == $repo and
    (.user.login == "dependabot[bot]" or
      .author_association == "OWNER" or
      .author_association == "MEMBER" or
      .author_association == "COLLABORATOR")
  ' <<<"$pull" > /dev/null || {
    echo 'Public repositories run Rady only for same-repository Dependabot or trusted collaborator pull requests' >&2
    exit 0
  }
fi
{
  echo 'allowed=true'
  echo "dependency=$dependency"
  echo "head=$(jq -r '.head.sha' <<<"$pull")"
  echo "number=$PR_NUMBER"
} >> "$GITHUB_OUTPUT"

#!/usr/bin/env bash

set -euo pipefail
matrix='[]'

emit() {
  jq -e 'type == "array" and length <= 100 and all(.[]; type == "number" and . > 0 and floor == .)' <<<"$matrix" > /dev/null
  {
    echo "matrix=$matrix"
    if [[ "$matrix" == '[]' ]]; then
      echo 'has-prs=false'
    else
      echo 'has-prs=true'
    fi
  } >> "$GITHUB_OUTPUT"
}

one() {
  [[ "$1" =~ ^[1-9][0-9]*$ ]] || return 0
  matrix=$(jq -cn --argjson number "$1" '[$number]')
}

backlog() {
  pulls='[]'
  for page in 1 2 3; do
    batch=$(gh api "repos/$GH_REPO/pulls?state=open&per_page=100&sort=created&direction=asc&page=$page")
    pulls=$(jq -ce --argjson batch "$batch" '. + $batch' <<<"$pulls")
    (( $(jq 'length' <<<"$batch") < 100 )) && break
  done
  if (( $(jq 'length' <<<"$batch") == 100 )); then
    extra=$(gh api "repos/$GH_REPO/pulls?state=open&per_page=1&sort=created&direction=asc&page=301")
    [[ "$extra" == '[]' ]] || { echo 'Rady backfill supports at most 300 open pull requests' >&2; exit 1; }
  fi
  matrix=$(jq -cer --arg repo "$GH_REPO" --argjson private "$PRIVATE_REPOSITORY" '
    [.[] | select(
      .draft == false and
      ($private or (
        .head.repo.full_name? == $repo and
        .base.repo.full_name? == $repo and
        (.user.login == "dependabot[bot]" or
          .author_association == "OWNER" or
          .author_association == "MEMBER" or
          .author_association == "COLLABORATOR")
      ))
    ) | .number]
  ' <<<"$pulls")
  (( $(jq 'length' <<<"$matrix") <= 100 )) || { echo 'Rady backfill supports at most 100 eligible pull requests' >&2; exit 1; }
}

case "$EVENT_NAME" in
  workflow_dispatch)
    if [[ -n "$PR_NUMBER" ]]; then one "$PR_NUMBER"; else backlog; fi
    ;;
  push|schedule) backlog ;;
esac
emit

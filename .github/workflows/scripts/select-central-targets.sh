#!/usr/bin/env bash
set -euo pipefail

: "${GH_TOKEN:?missing App installation token}"
: "${GITHUB_OUTPUT:?missing GitHub Actions output file}"
: "${RADY_APP_SLUG:?missing Rady App slug}"
: "${RADY_OWNER:?missing installation owner}"

maximum_repositories=100
maximum_targets=100
matrix='[]'

installation=$(gh api "installation/repositories?per_page=$maximum_repositories")
jq -e --arg owner "$RADY_OWNER" --argjson maximum "$maximum_repositories" '
  (.total_count | type == "number" and . <= $maximum) and
  (.repositories | type == "array" and length == .total_count) and
  all(.repositories[];
    .owner.login == $owner and
    (.full_name | type == "string" and test("^[A-Za-z0-9][A-Za-z0-9-]{0,38}/[A-Za-z0-9_.-]{1,100}$")) and
    (.name | type == "string" and test("^[A-Za-z0-9_.-]{1,100}$")) and
    (.private | type == "boolean")
  )
' <<<"$installation" > /dev/null

configuration_for() {
  local repository=$1 content decoded configuration
  if ! content=$(gh api "repos/$repository/contents/.github/rady.json" --jq '.content' 2>/dev/null); then
    printf '{"accepted":false,"checks":[]}'
    return
  fi
  if ! decoded=$(tr -d '\n' <<<"$content" | base64 --decode 2>/dev/null); then
    echo "::warning title=Invalid Rady configuration::$repository/.github/rady.json was ignored" >&2
    printf '{"accepted":false,"checks":[]}'
    return
  fi
  if ! configuration=$(jq -ce '
    select(.schema == 1) |
    select(.checks | type == "array" and length <= 32) |
    select(.checks | all(.[];
      type == "string" and
      length > 0 and length <= 200 and
      (startswith("Rady dependasolve") | not)
    )) |
    {
      accepted: (
        .agreement.terms == "2026-09-23" and
        .agreement.privacy == "2026-09-23" and
        (.agreement.accepted_by | type == "string" and length > 0 and length <= 100) and
        (.agreement.accepted_at_unix | type == "number" and . > 0 and floor == .) and
        (.agreement.issue | type == "number" and . > 0 and floor == .) and
        (.agreement.comment | type == "number" and . > 0 and floor == .)
      ),
      checks: (.checks | unique),
      agreement: .agreement
    }
  ' <<<"$decoded"); then
    echo "::warning title=Invalid Rady configuration::$repository/.github/rady.json was ignored" >&2
    printf '{"accepted":false,"checks":[]}'
    return
  fi
  if ! receipt_is_valid "$repository" "$configuration"; then
    printf '{"accepted":false,"checks":[]}'
    return
  fi
  printf '%s' "$configuration"
}

receipt_is_valid() {
  local repository=$1 configuration=$2 signer issue comment issue_value comment_value permission
  if [[ $(jq -r '.accepted' <<<"$configuration") != true ]]; then
    return 1
  fi
  if ! IFS=$'\t' read -r signer issue comment < <(jq -r '[.agreement.accepted_by, .agreement.issue, .agreement.comment] | @tsv' <<<"$configuration"); then
    return 1
  fi
  [[ "$signer" =~ ^[A-Za-z0-9-]{1,100}$ && "$issue" =~ ^[1-9][0-9]*$ && "$comment" =~ ^[1-9][0-9]*$ ]] || return 1
  if ! issue_value=$(gh api "repos/$repository/issues/$issue" 2>/dev/null); then
    return 1
  fi
  if ! comment_value=$(gh api "repos/$repository/issues/comments/$comment" 2>/dev/null); then
    return 1
  fi
  if ! permission=$(gh api "repos/$repository/collaborators/$signer/permission" 2>/dev/null); then
    return 1
  fi
  jq -e \
    --arg repository "$repository" \
    --arg signer "$signer" \
    --argjson issue "$issue" \
    --argjson comment "$comment" '
      .number == $issue and
      .html_url == ("https://github.com/" + $repository + "/issues/" + ($issue | tostring)) and
      .title == "Rady service agreement" and
      .state == "closed" and
      (has("pull_request") | not)
    ' <<<"$issue_value" >/dev/null || return 1
  jq -e \
    --arg repository "$repository" \
    --arg signer "$signer" \
    --argjson issue "$issue" \
    --argjson comment "$comment" '
      .id == $comment and
      .issue_url == ("https://api.github.com/repos/" + $repository + "/issues/" + ($issue | tostring)) and
      .user.login == $signer and
      .body == ("Rady service agreement acceptance\\n\\nI accept the Rady Terms of Use (2026-09-23) and Privacy Policy (2026-09-23) for " + $repository + ".")
    ' <<<"$comment_value" >/dev/null || return 1
  jq -e '.permission == "admin"' <<<"$permission" >/dev/null
}

while IFS=$'\t' read -r repository repository_name private; do
  configuration=$(configuration_for "$repository")
  if [[ $(jq -r '.accepted' <<<"$configuration") != true ]]; then
    echo "::notice title=Rady awaiting consent::$repository is paused until its current terms and privacy versions are accepted"
    continue
  fi
  checks=$(jq -c '.checks' <<<"$configuration")
  pulls='[]'
  for page in 1 2 3; do
    batch=$(gh api "repos/$repository/pulls?state=open&sort=created&direction=asc&per_page=100&page=$page")
    pulls=$(jq -ce --argjson batch "$batch" '. + $batch' <<<"$pulls")
    (( $(jq 'length' <<<"$batch") < 100 )) && break
  done
  jq -e --arg repository "$repository" '
    type == "array" and length <= 300 and all(.[];
      (.number | type == "number" and . > 0 and floor == .) and
      (.head.sha | type == "string" and test("^[a-f0-9]{40}$")) and
      .base.repo.full_name == $repository
    )
  ' <<<"$pulls" > /dev/null

  while IFS=$'\t' read -r number head; do
    reviews=$(gh api "repos/$repository/pulls/$number/reviews?per_page=100")
    if jq -e --arg login "${RADY_APP_SLUG}[bot]" --arg head "$head" '
      any(.[];
        .user.login == $login and
        .commit_id == $head and
        .state == "APPROVED"
      )
    ' <<<"$reviews" > /dev/null; then
      continue
    fi
    target=$(jq -cn \
      --arg repo "$repository" \
      --arg owner "$RADY_OWNER" \
      --arg name "$repository_name" \
      --argjson private "$private" \
      --argjson number "$number" \
      --argjson checks "$checks" \
      '{repo: $repo, owner: $owner, name: $name, private: $private, number: $number, checks: $checks}')
    matrix=$(jq -cn --argjson matrix "$matrix" --argjson target "$target" '$matrix + [$target]')
    (( $(jq 'length' <<<"$matrix") <= maximum_targets )) || {
      echo "Rady central review supports at most $maximum_targets pending pull requests" >&2
      exit 1
    }
  done < <(jq -r --arg repository "$repository" '
    .[] |
    select(
      .draft == false and
      .head.repo.full_name == $repository and
      (.user.login == "dependabot[bot]" or
        .author_association == "OWNER" or
        .author_association == "MEMBER" or
        .author_association == "COLLABORATOR")
    ) |
    [.number, .head.sha] | @tsv
  ' <<<"$pulls")
done < <(jq -r '
  .repositories |
  sort_by(.full_name)[] |
  select(.archived == false and .disabled == false) |
  [.full_name, .name, .private] | @tsv
' <<<"$installation")

jq -e --argjson maximum "$maximum_targets" '
  type == "array" and length <= $maximum and all(.[];
    (.repo | type == "string") and
    (.owner | type == "string") and
    (.name | type == "string") and
    (.private | type == "boolean") and
    (.number | type == "number" and . > 0 and floor == .) and
    (.checks | type == "array")
  )
' <<<"$matrix" > /dev/null
{
  printf 'matrix=%s\n' "$(jq -c . <<<"$matrix")"
  if [[ "$matrix" == '[]' ]]; then
    echo 'has-targets=false'
  else
    echo 'has-targets=true'
  fi
} >> "$GITHUB_OUTPUT"

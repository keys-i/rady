#!/usr/bin/env bash
set -euo pipefail

readonly script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly runner="$script_dir/gemini.sh"
readonly temp="$(mktemp -d)"
trap 'rm -rf -- "$temp"' EXIT
mkdir -p "$temp/bin" "$temp/runtime"
printf '%s\n' '#!/usr/bin/env bash' 'for arg in "$@"; do [[ "$arg" == --prompt ]] && found=1; done' '[[ "${found:-0}" == 1 ]] || exit 2' 'printf ran > "${SENTINEL:?}"' "printf '%s\\n' '{\"response\":\"sentinel\"}'" > "$temp/bin/gemini"
chmod +x "$temp/bin/gemini"

run() {
  (
    cd -- "$1"
    RUNNER_TEMP="$temp/runtime" GEMINI_API_KEY=test SENTINEL="$temp/ran" \
      PATH="$temp/bin:$PATH" bash "$runner"
  )
}

for unsafe in .gemini .env GEMINI.md; do
  workspace="$temp/$unsafe"
  mkdir -p "$workspace"
  if [[ "$unsafe" == .gemini ]]; then
    ln -s "$workspace/missing" "$workspace/$unsafe"
  else
    : > "$workspace/$unsafe"
  fi
  ! run "$workspace" > /dev/null 2>&1
  [[ ! -e "$temp/ran" ]]
done

workspace="$temp/clean"
mkdir -p "$workspace"
[[ "$(run "$workspace")" == sentinel ]]
[[ -e "$temp/ran" ]]

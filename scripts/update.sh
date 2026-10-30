#!/usr/bin/env bash
#
# Moves the SDK to the newest API definitions: API_REVISION to the head of
# origincloudhq/api's main, then generates the service crates' code again
# (scripts/generate.sh). The regeneration workflow
# (.github/workflows/regenerate.yml) runs it daily; run it by hand the same
# way.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly ROOT
readonly API_REPOSITORY="https://github.com/origincloudhq/api.git"

cd "${ROOT}"

revision="$(git ls-remote "${API_REPOSITORY}" refs/heads/main | cut -f1)"
[[ "${revision}" =~ ^[0-9a-f]{40}$ ]] || { echo "no main branch in ${API_REPOSITORY}" >&2; exit 1; }
echo "${revision}" > API_REVISION

echo "origincloudhq/api@${revision:0:12}"
scripts/generate.sh

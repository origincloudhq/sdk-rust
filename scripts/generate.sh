#!/usr/bin/env bash
#
# Generates the service crates' code from the API definitions in
# origincloudhq/api at the commit API_REVISION names, with this repository's
# generator (generator/), built at the versions Cargo.lock pins: the
# messages, their JSON mapping and the clients of each proto package, in
# crates/<service>/src/generated/ and crates/core/src/longrunning/generated/.
#
# A service the API gains gets its crate scaffolded: crates/<service>'s
# Cargo.toml and src/lib.rs, from the generator's template with the
# description the API's definitions give, a place among the workspace's
# members (Cargo.toml, Cargo.lock), and a row in README.md's table.
#
#   scripts/generate.sh           # generate it again
#   scripts/generate.sh --check   # check the checked-in code is current
#
# Needs cargo, git and network access: the API's dependencies are
# downloaded from the Buf Schema Registry at the commits its buf.lock pins,
# and checked against the digests it pins.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly ROOT
readonly API_REPOSITORY="https://github.com/origincloudhq/api.git"

case "${1:-}" in
  "") mode="write" ;;
  --check) mode="check" ;;
  *) echo "usage: $0 [--check]" >&2; exit 2 ;;
esac

revision="$(tr -d '[:space:]' < "${ROOT}/API_REVISION")"
[[ "${revision}" =~ ^[0-9a-f]{40}$ ]] || { echo "API_REVISION must hold a full commit hash" >&2; exit 1; }

tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT

git init --quiet "${tmp}/api"
git -C "${tmp}/api" fetch --quiet --depth 1 "${API_REPOSITORY}" "${revision}"
git -C "${tmp}/api" checkout --quiet FETCH_HEAD
(cd "${ROOT}" && cargo run --quiet --locked -p origincloud-sdk-generator -- "${tmp}/api" "${tmp}/gen")

# The generated files, relative to the repository.
generated() { (cd "$1" && find crates -path '*/generated/*' -type f | LC_ALL=C sort); }
# The files scaffolding new crates, which the generator writes only for
# a service without a crate.
scaffolded() { (cd "$1" && find . -type f -not -path '*/generated/*' | sed 's|^\./||' | LC_ALL=C sort); }

if [[ "${mode}" == write ]]; then
  generated "${ROOT}" | (cd "${ROOT}" && xargs -r rm -f)
  (cd "${tmp}/gen" && find crates -path '*/generated/*' -type f -exec cp --parents {} "${ROOT}" \;)
  new="$(scaffolded "${tmp}/gen")"
  if [[ -n "${new}" ]]; then
    (cd "${tmp}/gen" && xargs cp --parents -t "${ROOT}" <<< "${new}")
    (cd "${ROOT}" && cargo update --quiet --workspace)
    echo "scaffolded:"
    while read -r file; do echo "  ${file}"; done <<< "${new}"
  fi
  echo "wrote $(generated "${ROOT}" | wc -l) files from origincloudhq/api@${revision:0:12}"
  exit 0
fi

if [[ -n "$(scaffolded "${tmp}/gen")" ]]; then
  echo "FAILED: origincloudhq/api@${revision:0:12} has a service without a crate; run $0 to scaffold it"
  exit 1
fi

failed=0
if ! diff <(generated "${ROOT}") <(generated "${tmp}/gen") >/dev/null; then
  failed=1
else
  while read -r file; do
    cmp -s "${ROOT}/${file}" "${tmp}/gen/${file}" || failed=1
  done < <(generated "${ROOT}")
fi
if [[ "${failed}" -eq 1 ]]; then
  echo "FAILED: the generated code isn't what origincloudhq/api@${revision:0:12} generates; run $0"
  exit 1
fi
echo "ok - the generated code is current (origincloudhq/api@${revision:0:12})"

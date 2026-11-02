#!/usr/bin/env bash
#
# Tests scripts/version.sh against a throwaway repository with tags of its
# own: CI runs it.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly ROOT

tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT
mkdir -p "${tmp}/scripts" "${tmp}/crates/core" "${tmp}/crates/pvs"
cp "${ROOT}/scripts/version.sh" "${tmp}/scripts/"
git -C "${tmp}" init --quiet
git -C "${tmp}" config user.email test@example.com
git -C "${tmp}" config user.name test

failed=0
expect() {
  local what="$1" want="$2" got="$3"
  if [[ "${got}" != "${want}" ]]; then
    echo "not ok - ${what}: want ${want}, got ${got}"
    failed=1
  else
    echo "ok - ${what}"
  fi
}
set_version() {
  printf '[package]\nname = "origincloud-core"\nversion = "%s"\n\n[dependencies]\nserde = { version = "1" }\n' "$1" \
    > "${tmp}/crates/core/Cargo.toml"
  printf '[package]\nname = "origincloud-pvs"\nversion = "%s"\n\n[dependencies]\norigincloud-core = { version = "%s", path = "../core" }\n\n[target.x.dependencies]\nversion = "9"\n' "$1" "$1" \
    > "${tmp}/crates/pvs/Cargo.toml"
}
v() { "${tmp}/scripts/version.sh" "$@"; }
released() { if v unreleased; then echo unreleased; else echo released; fi; }

set_version 0.1.0
git -C "${tmp}" add -A && git -C "${tmp}" commit --quiet -m one
expect "a version before the first tag is unreleased" unreleased "$(released)"
git -C "${tmp}" tag -a v0.1.0 -m v0.1.0
expect "the current version" 0.1.0 "$(v current)"
expect "a tagged version is released" released "$(released)"

expect "a patch after the newest tag" 0.1.1 "$(v bump patch)"
expect "it is written" 0.1.1 "$(v current)"
pvs="$(cat "${tmp}/crates/pvs/Cargo.toml")"
expect "every crate's version" 'version = "0.1.1"' "$(grep -m 1 '^version' <<< "${pvs}")"
expect "its core dependency" 'origincloud-core = { version = "0.1.1", path = "../core" }' "$(grep '^origincloud-core' <<< "${pvs}")"
expect "no other version line" 'version = "9"' "$(grep '^version' <<< "${pvs}" | tail -n 1)"
expect "the bump is unreleased" unreleased "$(released)"
expect "a second patch keeps the pending one" 0.1.1 "$(v bump patch)"
expect "a minor replaces a pending patch" 0.2.0 "$(v bump minor)"
expect "a patch keeps a pending minor" 0.2.0 "$(v bump patch)"

git -C "${tmp}" tag -a v0.2.0 -m v0.2.0
git -C "${tmp}" tag -a v0.10.0 -m v0.10.0
set_version 0.10.0
expect "tags order by version, not as text" 0.11.0 "$(v bump minor)"

set_version 0.9.0
expect "a version older than the newest tag isn't unreleased" released "$(released)"

# Everything above runs without origin/main, as in a clone that hasn't
# fetched it: bump counts from the newest tag alone. Below, origin/main
# holds the version the last merged pull request set, as the regeneration
# workflow's checkout has it.
main() {
  set_version "$1"
  git -C "${tmp}" commit --quiet -am "main at $1"
  git -C "${tmp}" update-ref refs/remotes/origin/main HEAD
}

main 0.11.0
expect "main's version newer than the newest tag: a patch after main's" 0.11.1 "$(v bump patch)"
expect "a minor after main's" 0.12.0 "$(v bump minor)"
git -C "${tmp}" commit --quiet -am "the branch's bump"
expect "a branch that bumped already isn't bumped again" 0.12.0 "$(v bump minor)"
expect "nor by a patch" 0.12.0 "$(v bump patch)"

main 0.11.0
git -C "${tmp}" tag -a v0.11.0 -m v0.11.0
expect "main at the newest tag: a minor after it" 0.12.0 "$(v bump minor)"
set_version 0.11.0
expect "a patch after it" 0.11.1 "$(v bump patch)"

git -C "${tmp}" update-ref refs/remotes/origin/main HEAD~3
set_version 0.11.0
expect "main older than the newest tag: a patch after the tag" 0.11.1 "$(v bump patch)"

if v bump major >/dev/null 2>&1; then
  echo "not ok - bump refuses a kind it doesn't know"
  failed=1
else
  echo "ok - bump refuses a kind it doesn't know"
fi
exit "${failed}"

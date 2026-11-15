#!/usr/bin/env bash
#
# Checks the SDK for the browser, wasm32-unknown-unknown: the published
# crates build and are lint-free there, and the core crate's wasm tests
# (crates/core/tests/wasm.rs) pass, run by wasm-bindgen-test in Node.js,
# whose fetch, setTimeout and Date stand in for a browser's.
#
#   scripts/wasm.sh
#
# Needs the wasm32-unknown-unknown target (rustup target add
# wasm32-unknown-unknown), Node.js, and wasm-bindgen-test-runner at the
# version of wasm-bindgen Cargo.lock pins, as the runner refuses tests
# built with another: on PATH, or else downloaded from wasm-bindgen's
# release, checked against its checksum, into target/wasm-bindgen.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly ROOT
readonly TARGET="wasm32-unknown-unknown"
readonly EXCLUDE=(--exclude origincloud-sdk-generator --exclude fake-server)

cd "${ROOT}"

cargo check --locked --target "${TARGET}" --workspace "${EXCLUDE[@]}"
cargo clippy --locked --target "${TARGET}" --workspace "${EXCLUDE[@]}" -- -D warnings
cargo clippy --locked --target "${TARGET}" -p origincloud-core --test wasm -- -D warnings

# The version of wasm-bindgen the lockfile pins: the line after its name.
version="$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/[^0-9.]/, ""); print }' Cargo.lock)"
[[ "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "no wasm-bindgen version in Cargo.lock" >&2; exit 1; }

if [[ "$(wasm-bindgen-test-runner --version 2>/dev/null || true)" != "wasm-bindgen-test-runner ${version}" ]]; then
  name="wasm-bindgen-${version}-x86_64-unknown-linux-musl"
  dir="${CARGO_TARGET_DIR:-${ROOT}/target}/wasm-bindgen"
  if [[ ! -x "${dir}/${name}/wasm-bindgen-test-runner" ]]; then
    url="https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${version}/${name}.tar.gz"
    mkdir -p "${dir}"
    curl -fsSL -o "${dir}/${name}.tar.gz" "${url}"
    want="$(curl -fsSL "${url}.sha256sum" | cut -d ' ' -f 1)"
    echo "${want}  ${dir}/${name}.tar.gz" | sha256sum --check --quiet
    tar -xzf "${dir}/${name}.tar.gz" -C "${dir}"
  fi
  PATH="${dir}/${name}:${PATH}"
fi

CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
  cargo test --locked --target "${TARGET}" -p origincloud-core --test wasm

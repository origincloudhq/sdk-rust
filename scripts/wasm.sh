#!/usr/bin/env bash
#
# Checks the SDK for the browser, wasm32-unknown-unknown: the published
# crates build, are lint-free and document without broken links there,
# with and without the core crate's feature service-account-wasm, and the
# core crate's wasm tests (crates/core/tests/wasm.rs) pass, run by
# wasm-bindgen-test in Node.js, whose fetch, setTimeout and Date stand in
# for a browser's.
#
#   scripts/wasm.sh
#
# Needs the wasm32-unknown-unknown target (rustup target add
# wasm32-unknown-unknown), Node.js, and wasm-bindgen-test-runner at the
# version of wasm-bindgen Cargo.lock pins, as the runner refuses tests
# built with another: on PATH, or else built from crates.io with
# `cargo install --locked` into WASM_BINDGEN_ROOT (by default
# target/wasm-bindgen; CI uses ~/.cargo, which its cache keeps).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly ROOT
readonly TARGET="wasm32-unknown-unknown"
readonly CRATES=(--workspace --exclude origincloud-sdk-generator --exclude fake-server)
readonly FEATURE=(--features origincloud-core/service-account-wasm)

cd "${ROOT}"

check() {
  cargo check --locked --target "${TARGET}" "${CRATES[@]}" "$@"
  cargo clippy --locked --target "${TARGET}" "${CRATES[@]}" "$@" -- -D warnings
  RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --target "${TARGET}" "${CRATES[@]}" "$@"
}
check
check "${FEATURE[@]}"
cargo clippy --locked --target "${TARGET}" -p origincloud-core "${FEATURE[@]}" --test wasm -- -D warnings

# The version of wasm-bindgen the lockfile pins: the line after its name.
version="$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/[^0-9.]/, ""); print }' Cargo.lock)"
[[ "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "no wasm-bindgen version in Cargo.lock" >&2; exit 1; }
want="wasm-bindgen-test-runner ${version}"

if [[ "$(wasm-bindgen-test-runner --version 2>/dev/null || true)" != "${want}" ]]; then
  root="${WASM_BINDGEN_ROOT:-${CARGO_TARGET_DIR:-${ROOT}/target}/wasm-bindgen}"
  if [[ "$("${root}/bin/wasm-bindgen-test-runner" --version 2>/dev/null || true)" != "${want}" ]]; then
    cargo install --locked --force --root "${root}" wasm-bindgen-cli --version "=${version}"
  fi
  PATH="${root}/bin:${PATH}"
fi

CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
  cargo test --locked --target "${TARGET}" -p origincloud-core "${FEATURE[@]}" --test wasm

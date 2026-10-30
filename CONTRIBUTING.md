# Contributing

Thank you for helping improve Origin Cloud's SDK for Rust.

## Issues

Open an issue for anything that is wrong, unclear or missing: a method that
behaves differently from its description, an option you need, documentation that
doesn't explain enough. Say which crate and method it is about, and include
the request and response where you can, with tokens and account details
removed.

To report a security vulnerability, don't open an issue: use **Report a
vulnerability** on this repository's Security tab, which reaches us privately.

## Pull requests

Pull requests are reviewed and merged here. For anything larger than a small
fix, open an issue first, so that we can agree on the change before you spend
time on it. CI must pass: the checks below, run on every pull request.

## How the SDK is built

The service crates' code (`crates/<service>/src/generated/`, and the core
crate's `src/longrunning/generated/`) is generated from the API definitions in
[origincloudhq/api](https://github.com/origincloudhq/api), at the commit
`API_REVISION` names, by this repository's generator, `generator/`:

- the API's dependencies are downloaded from the Buf Schema Registry at the
  commits its `buf.lock` pins, and checked against the digests it pins;
- [protox](https://docs.rs/protox) compiles the definitions;
- [prost-build](https://docs.rs/prost-build) writes the messages,
  [pbjson-build](https://docs.rs/pbjson-build) their proto3 JSON mapping, and the
  generator itself the clients, one per service, from each method's
  `google.api.http` binding.

Everything it runs is pinned by `Cargo.lock`, so generating from the same API
always gives the same code. `origincloud.{service}.{version}` becomes the
module `{version}` of the crate `origincloud-{service}` in `crates/{service}`;
`origincloud.longrunning.{version}` the core crate's
`longrunning::{version}`. The rest of the core crate is written by hand.

So a change to what a service offers starts in the API, which is why it is
requested by opening an issue rather than by editing generated code; a change
to how every client behaves is a change to the generator or the core crate. A
new version of a service appears by itself. So does a new service: the
generator scaffolds its crate, a `crates/<service>/Cargo.toml` and `src/lib.rs`
from a template matching the PVS crate's, adds it to the workspace, and says
so; review the scaffolded description and keywords before it is merged.

The regeneration workflow (`.github/workflows/regenerate.yml`) moves the SDK
to the newest API daily, and opens a pull request when that changes anything.

## Checks

Rust (the version `rust-toolchain.toml` names), git and network access are all
you need.

```sh
scripts/generate.sh            # generate the code again
scripts/generate.sh --check    # check the checked-in code is what the API generates
scripts/update.sh              # move to the newest API, and generate
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                     # the live tests are ignored
ORIGINCLOUD_LIVE_ZONE=projects/<project>/zones/<zone> \
  cargo test -p origincloud-pvs --test live -- --ignored --nocapture   # against the live API
```

CI also builds the documentation and checks the scripts with shellcheck.
Releases are described in [RELEASING.md](RELEASING.md).

## License

By contributing, you agree that your contribution is licensed under the
Apache License 2.0, like the rest of this repository.

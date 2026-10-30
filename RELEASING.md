# Releasing

A release publishes every crate of the workspace to crates.io at one version,
from a commit on `main` whose CI passed, and tags that commit. It is done by
hand, by an owner of the crates on crates.io.

1. Move the version: set `version` in `crates/*/Cargo.toml`, and the
   `origincloud-core` dependency of each service crate, to the next version,
   then run `cargo update --workspace`. Open a pull request with it and merge
   it once CI passes.

2. Check what will be published, on that commit:

   ```sh
   git switch --detach origin/main
   cargo package --locked --workspace --exclude origincloud-sdk-generator --exclude fake-server
   ```

3. Publish the core crate first, then the service crates, which depend on it.
   `cargo publish` needs a crates.io API token (`cargo login`) of an owner:

   ```sh
   cargo publish --locked -p origincloud-core
   cargo publish --locked -p origincloud-pvs
   ```

4. Tag the commit:

   ```sh
   git tag -a v0.2.0 -m v0.2.0 origin/main
   git push origin v0.2.0
   ```

Then check that it resolves: `cargo info origincloud-pvs@0.2.0`.

The first release of a new crate makes its publisher its only owner; add the
organisation's crates.io team as an owner then (`cargo owner --add`).

Versions follow semantic versioning. Before 1.0, a minor version may change
the API incompatibly, as the preview APIs themselves may (a `v1beta1`
module). A version is never replaced once released: crates.io keeps it, and
a broken one is yanked (`cargo yank`), not deleted.

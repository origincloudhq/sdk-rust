# Releasing

A release publishes every crate of the workspace to crates.io at one version,
the `version` of `crates/*/Cargo.toml` and of the `origincloud-core` each
service crate depends on, from a commit on `main` whose CI passed, and tags
that commit `v<version>`.

Versions follow semantic versioning. Before 1.0, a minor version may change
the API incompatibly, as the preview APIs themselves may (a `v1beta1`
module). A version is never replaced once released: crates.io keeps it, and
a broken one is yanked (`cargo yank`), not deleted.

## The automatic path

1. **Regeneration.** When the API changes, `regenerate.yml` opens or refreshes
   the pull request from the branch `regenerate`, and sets the next version in
   it with `scripts/version.sh bump`: a minor version when `API_REVISION` moved,
   a patch for a change of the generator alone.
2. **Merge.** Once CI passes on that pull request's head commit,
   `merge-regeneration.yml` merges it, if every commit in it is the
   regeneration workflow's. A pull request someone added commits to waits for
   review.
3. **Release.** Once CI passes on `main`, `release.yml` dispatches itself for
   that commit, because crates.io's trusted publishing refuses a run started
   by another workflow's, and publishes the version if it has no tag yet (`scripts/version.sh unreleased`): the core crate first,
   then the service crates, skipping any whose version is on crates.io
   already, so that a release that failed half way finishes when it runs
   again. Then it tags the commit, and dispatches origincloudhq/mcp's
   regeneration.

It publishes with crates.io's trusted publishing, from the environment
`release`: no crates.io token is stored here. The merge and the dispatch use
the regeneration App's token, whose ID is the variable `REGENERATE_APP_ID` and
whose key is the secret `REGENERATE_APP_PRIVATE_KEY`; a merge with the workflow
token would start no CI on `main`, and so no release. Without the App, step 1
still opens the pull request, and the merge is done by hand.

## A new crate

Trusted publishing reaches only crates that exist, so a crate's first version
is published by hand, by an owner of the organisation on crates.io; until then
the release publishes the crates that exist, the core crate among them, and
fails, naming the new crate, without tagging. The existing crates accept only
trusted publishing, so the core version a new crate depends on comes from that
release. For each new crate:

1. Once the release has failed naming it, publish its first version from the
   commit on `main` that has it, with a crates.io API token (`cargo login`):

   ```sh
   git switch --detach origin/main
   cargo publish --locked -p origincloud-<service>
   ```

2. Add the organisation's crates.io team as an owner (`cargo owner --add`): the
   first publish makes its publisher the only one.
3. On crates.io, add this repository as the crate's trusted publisher: workflow
   `release.yml`, environment `release`.
4. Run `release.yml` again (re-run its last run), which publishes the other
   crates and tags the commit.

## By hand

A change outside a regeneration is released by setting the version in the
same pull request: `scripts/version.sh bump minor` or `scripts/version.sh bump
patch`, then `cargo update --workspace`. `release.yml` publishes it once the
pull request is merged and CI passes on `main`.

Without the workflow, an owner publishes and tags from that commit:

```sh
git switch --detach origin/main
cargo package --locked --workspace --exclude origincloud-sdk-generator --exclude fake-server
cargo publish --locked -p origincloud-core
cargo publish --locked -p origincloud-pvs
cargo publish --locked -p origincloud-location
git tag -a v0.2.0 -m v0.2.0 origin/main
git push origin v0.2.0
```

Then check that it resolves: `cargo info origincloud-pvs@0.2.0`.

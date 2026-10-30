//! A new service's crate: when the API gains a service that has no crate
//! yet, its `Cargo.toml` and `src/lib.rs` are written from the template
//! below, which matches the PVS crate's, and the crate is added to the
//! workspace, so that generating it never stops for want of the crate.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const MANIFEST: &str = r#"[package]
name = "origincloud-{service}"
version = "{version}"
description = "The Origin Cloud {SERVICE} API."
keywords = ["origincloud", "cloud", "sdk", "api"]
categories = ["api-bindings", "web-programming::http-client"]
readme = "../../README.md"
edition.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true

[dependencies]
origincloud-core = { version = "{version}", path = "../core" }
pbjson = "0.9"
prost = "0.14"
serde = "1"
"#;

const LIB: &str = r#"//! The Origin Cloud {SERVICE} API (`origincloud.{service}`).
//!
//! The clients are generated from the API definitions; what they share,
//! such as credentials, retries and errors, is in [`origincloud_core`].

mod generated;

pub use generated::*;
"#;

/// The files that add the crates of `services` to the repository at
/// `repository`: each crate's manifest and `src/lib.rs`, and the
/// workspace manifest with the crates among its members.
pub fn crates(repository: &Path, services: &[String]) -> Result<Vec<(PathBuf, String)>> {
    if services.is_empty() {
        return Ok(Vec::new());
    }
    let core = std::fs::read_to_string(repository.join("crates/core/Cargo.toml"))
        .context("reading the core crate's manifest")?;
    let version = package_version(&core).context("the core crate's manifest names no version")?;
    let workspace = std::fs::read_to_string(repository.join("Cargo.toml"))
        .context("reading the workspace manifest")?;

    let mut files = Vec::new();
    let mut members = Vec::new();
    for service in services {
        let fill = |template: &str| {
            template
                .replace("{service}", service)
                .replace("{SERVICE}", &service.to_uppercase())
                .replace("{version}", version)
        };
        let dir = PathBuf::from("crates").join(service);
        files.push((dir.join("Cargo.toml"), fill(MANIFEST)));
        files.push((dir.join("src").join("lib.rs"), fill(LIB)));
        members.push(format!("crates/{service}"));
    }
    files.push((
        PathBuf::from("Cargo.toml"),
        with_members(&workspace, &members)?,
    ));
    Ok(files)
}

/// The `version` of a manifest's `[package]`.
fn package_version(manifest: &str) -> Option<&str> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package && let Some(value) = line.strip_prefix("version = ") {
            return Some(value.trim_matches('"'));
        }
    }
    None
}

/// The workspace manifest with `added` among its members, which it lists on
/// one line, sorted.
fn with_members(workspace: &str, added: &[String]) -> Result<String> {
    let mut out = String::with_capacity(workspace.len() + 64);
    let mut found = false;
    for line in workspace.split_inclusive('\n') {
        let Some(list) = line
            .trim_end()
            .strip_prefix("members = [")
            .and_then(|l| l.strip_suffix(']'))
        else {
            out.push_str(line);
            continue;
        };
        found = true;
        let mut members: Vec<String> = list
            .split(',')
            .map(|m| m.trim().trim_matches('"').to_owned())
            .filter(|m| !m.is_empty())
            .collect();
        members.extend(added.iter().cloned());
        members.sort();
        members.dedup();
        let quoted: Vec<String> = members.iter().map(|m| format!("{m:?}")).collect();
        out.push_str(&format!("members = [{}]\n", quoted.join(", ")));
    }
    if !found {
        bail!("the workspace manifest has no one-line `members = [...]` to add the new crates to");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
    }

    /// The template is the PVS crate's manifest, but for its name,
    /// description, keywords and tests.
    #[test]
    fn the_template_matches_the_pvs_crate() {
        let pvs = std::fs::read_to_string(repository().join("crates/pvs/Cargo.toml")).unwrap();
        let normalize = |manifest: &str| -> Vec<String> {
            manifest
                .split("[dev-dependencies]")
                .next()
                .unwrap()
                .trim_end()
                .lines()
                .filter(|l| !l.starts_with("description = ") && !l.starts_with("keywords = "))
                .map(str::to_owned)
                .collect()
        };
        let files = crates(repository(), &["pvs".to_owned()]).unwrap();
        assert_eq!(normalize(&files[0].1), normalize(&pvs));
        let lib = std::fs::read_to_string(repository().join("crates/pvs/src/lib.rs")).unwrap();
        assert!(lib.contains("mod generated;\n\npub use generated::*;\n"));
        assert!(
            files[1]
                .1
                .ends_with("mod generated;\n\npub use generated::*;\n")
        );
    }

    #[test]
    fn a_new_crate_joins_the_workspace_members_sorted() {
        let workspace = "[workspace]\nresolver = \"3\"\nmembers = [\"crates/core\", \"crates/pvs\", \"generator\"]\n\n[workspace.package]\n";
        assert_eq!(
            with_members(workspace, &["crates/compute".to_owned()]).unwrap(),
            "[workspace]\nresolver = \"3\"\nmembers = [\"crates/compute\", \"crates/core\", \"crates/pvs\", \"generator\"]\n\n[workspace.package]\n"
        );
        assert!(with_members("[workspace]\n", &["crates/x".to_owned()]).is_err());
    }

    #[test]
    fn the_version_is_the_package_s() {
        let manifest =
            "[package]\nname = \"x\"\nversion = \"0.3.1\"\n\n[dependencies]\nversion = \"9\"\n";
        assert_eq!(package_version(manifest), Some("0.3.1"));
    }
}

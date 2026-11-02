//! A new service's crate: when the API gains a service that has no crate
//! yet, its `Cargo.toml` and `src/lib.rs` are written from the template
//! below, which matches the PVS crate's, with the description and keywords
//! its definitions give (`describe`); the crate is added to the workspace,
//! and to README.md's table of crates, so that generating it never stops
//! for want of the crate, and the crate is ready to publish.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::describe::Description;

const MANIFEST: &str = r#"[package]
name = "origincloud-{service}"
version = "{version}"
description = {description}
keywords = {keywords}
categories = ["api-bindings", "web-programming::http-client"]
readme = "../../README.md"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true

[dependencies]
origincloud-core = { version = "{version}", path = "../core" }
pbjson = "0.9"
prost = "0.14"
serde = "1"
"#;

const LIB: &str = r#"//! {summary}
//!
//! The clients are generated from the API definitions, `origincloud.{service}`;
//! what they share, such as credentials, retries and errors, is in
//! [`origincloud_core`].

mod generated;

pub use generated::*;
"#;

/// The files that add the crates of `services` to the repository at
/// `repository`: each crate's manifest and `src/lib.rs`, the workspace
/// manifest with the crates among its members, and README.md with their
/// rows in its table of crates.
pub fn crates(repository: &Path, services: &[Description]) -> Result<Vec<(PathBuf, String)>> {
    if services.is_empty() {
        return Ok(Vec::new());
    }
    let core = std::fs::read_to_string(repository.join("crates/core/Cargo.toml"))
        .context("reading the core crate's manifest")?;
    let version = package_version(&core).context("the core crate's manifest names no version")?;
    let workspace = std::fs::read_to_string(repository.join("Cargo.toml"))
        .context("reading the workspace manifest")?;
    let readme =
        std::fs::read_to_string(repository.join("README.md")).context("reading README.md")?;

    let mut files = Vec::new();
    let mut members = Vec::new();
    let mut rows = Vec::new();
    for description in services {
        let service = &description.service;
        let keywords: Vec<String> = description
            .keywords()
            .iter()
            .map(|k| toml_string(k))
            .collect();
        let values = [
            ("description", toml_string(&description.summary())),
            ("keywords", format!("[{}]", keywords.join(", "))),
            ("summary", description.summary()),
            ("service", service.clone()),
            ("version", version.to_owned()),
        ];
        let fill = |template: &str| fill(template, &values);
        let dir = PathBuf::from("crates").join(service);
        files.push((dir.join("Cargo.toml"), fill(MANIFEST)));
        files.push((dir.join("src").join("lib.rs"), fill(LIB)));
        members.push(format!("crates/{service}"));
        rows.push(description.readme_row());
    }
    files.push((
        PathBuf::from("Cargo.toml"),
        with_members(&workspace, &members)?,
    ));
    files.push((PathBuf::from("README.md"), with_rows(&readme, &rows)?));
    Ok(files)
}

/// template with each `{name}` of values replaced, in one pass, so that a
/// value holding braces, such as a description naming
/// `projects/{project}`, stays as it is.
fn fill(template: &str, values: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let value = rest.find('}').and_then(|end| {
            values
                .iter()
                .find(|(name, _)| *name == &rest[1..end])
                .map(|(_, value)| (end, value))
        });
        match value {
            Some((end, value)) => {
                out.push_str(value);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('{');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// text as a TOML basic string.
fn toml_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// README.md with `rows` added at the end of its table of crates, the one
/// headed `| Crate | API |`; a row for a crate it has is left out.
fn with_rows(readme: &str, rows: &[String]) -> Result<String> {
    let lines: Vec<&str> = readme.split_inclusive('\n').collect();
    let header = lines
        .iter()
        .position(|l| l.trim_end() == "| Crate | API |")
        .context(
            "README.md has no table of crates, headed `| Crate | API |`, to add the new crates to",
        )?;
    let end = lines[header..]
        .iter()
        .position(|l| !l.starts_with('|'))
        .map_or(lines.len(), |n| header + n);
    let mut out: String = lines[..end].concat();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    for row in rows {
        let name = row.split(" | ").next().unwrap_or(row);
        if !lines[header..end].iter().any(|l| l.starts_with(name)) {
            out.push_str(row);
            out.push('\n');
        }
    }
    out.push_str(&lines[end..].concat());
    Ok(out)
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
        let files = crates(repository(), &[description("pvs")]).unwrap();
        assert_eq!(normalize(&files[0].1), normalize(&pvs));
        let lib = std::fs::read_to_string(repository().join("crates/pvs/src/lib.rs")).unwrap();
        assert!(lib.contains("mod generated;\n\npub use generated::*;\n"));
        assert!(
            files[1]
                .1
                .ends_with("mod generated;\n\npub use generated::*;\n")
        );
    }

    fn description(service: &str) -> Description {
        Description {
            service: service.to_owned(),
            what: "instances and \"key pairs\" in projects/{project}".to_owned(),
            versions: vec!["v1beta1".to_owned()],
        }
    }

    /// The manifest and the documentation say what the definitions do,
    /// and the crate gets its row in README.md. The service is one the
    /// repository won't have, so that the test passes once it has more.
    #[test]
    fn a_new_crate_is_described() {
        let files = crates(repository(), &[description("example")]).unwrap();
        let paths: Vec<_> = files.iter().map(|(p, _)| p.to_str().unwrap()).collect();
        assert_eq!(
            paths,
            [
                "crates/example/Cargo.toml",
                "crates/example/src/lib.rs",
                "Cargo.toml",
                "README.md"
            ]
        );
        assert!(files[0].1.contains(
            "description = \"The Origin Cloud example API: instances and \\\"key pairs\\\" in projects/{project}.\"\n\
             keywords = [\"origincloud\", \"cloud\", \"sdk\", \"api\", \"example\"]\n"
        ));
        assert!(files[1].1.starts_with(
            "//! The Origin Cloud example API: instances and \"key pairs\" in projects/{project}.\n//!\n"
        ));
        assert!(files[3].1.contains(
            "| [`origincloud-example`](crates/example) | Instances and \"key pairs\" in projects/{project} (`origincloud_example::v1beta1`) |\n"
        ));
    }

    #[test]
    fn a_new_crate_s_row_ends_the_table_of_crates() {
        let readme = "# SDK\n\n| Crate | API |\n|---|---|\n| [`origincloud-pvs`](crates/pvs) | Volumes |\n\n## Example\n";
        let rows = [
            "| [`origincloud-pvs`](crates/pvs) | Again |".to_owned(),
            "| [`origincloud-compute`](crates/compute) | Instances |".to_owned(),
        ];
        assert_eq!(
            with_rows(readme, &rows).unwrap(),
            "# SDK\n\n| Crate | API |\n|---|---|\n| [`origincloud-pvs`](crates/pvs) | Volumes |\n\
             | [`origincloud-compute`](crates/compute) | Instances |\n\n## Example\n"
        );
        assert!(with_rows("# SDK\n", &rows).is_err());
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

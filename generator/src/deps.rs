//! The API's dependencies, as its `buf.lock` pins them: each module is
//! downloaded from its registry at the locked commit and checked against
//! the locked digest before it is compiled.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use sha3::Shake256;
use sha3::digest::{ExtendableOutput, Update, XofReader};

/// One module `buf.lock` pins.
#[derive(Debug, Default)]
struct Locked {
    /// Such as `buf.build/googleapis/googleapis`.
    name: String,
    commit: String,
    /// Such as `b5:0318e7…`.
    digest: String,
}

/// Returns the files of every module the API depends on, by their import
/// path.
pub fn fetch(api: &Path) -> Result<BTreeMap<String, String>> {
    let lock = std::fs::read_to_string(api.join("buf.lock")).context("reading buf.lock")?;
    let mut files = BTreeMap::new();
    for module in parse_lock(&lock)? {
        for (path, content) in download(&module)? {
            if path.ends_with(".proto") {
                let source = String::from_utf8(content)
                    .with_context(|| format!("{path} in {} isn't UTF-8", module.name))?;
                files.insert(path, source);
            }
        }
    }
    Ok(files)
}

/// Reads the `deps` of a version 2 `buf.lock`.
fn parse_lock(lock: &str) -> Result<Vec<Locked>> {
    let mut modules: Vec<Locked> = Vec::new();
    let mut version = None;
    for line in lock.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("version:") {
            version = Some(v.trim().to_owned());
        } else if let Some(name) = line.strip_prefix("- name:") {
            modules.push(Locked {
                name: name.trim().to_owned(),
                ..Locked::default()
            });
        } else if let Some(commit) = line.strip_prefix("commit:") {
            let module = modules
                .last_mut()
                .context("buf.lock: a commit outside a dependency")?;
            module.commit = commit.trim().to_owned();
        } else if let Some(digest) = line.strip_prefix("digest:") {
            let module = modules
                .last_mut()
                .context("buf.lock: a digest outside a dependency")?;
            module.digest = digest.trim().to_owned();
        }
    }
    ensure!(
        version.as_deref() == Some("v2"),
        "buf.lock: version {version:?} isn't supported, only v2"
    );
    for module in &modules {
        ensure!(
            !module.commit.is_empty() && module.digest.starts_with("b5:"),
            "buf.lock: {} needs a commit and a b5 digest",
            module.name
        );
    }
    Ok(modules)
}

#[derive(Deserialize)]
struct DownloadResponse {
    contents: Vec<Content>,
}

#[derive(Deserialize)]
struct Content {
    commit: Commit,
    files: Vec<DownloadedFile>,
}

#[derive(Deserialize)]
struct Commit {
    id: String,
}

#[derive(Deserialize)]
struct DownloadedFile {
    path: String,
    /// Base64.
    content: String,
}

/// Downloads a module at its locked commit through the registry's
/// DownloadService, and checks its files against the locked digest.
fn download(module: &Locked) -> Result<BTreeMap<String, Vec<u8>>> {
    let Some((registry, _)) = module.name.split_once('/') else {
        bail!(
            "buf.lock: {} isn't a registry/owner/module name",
            module.name
        );
    };
    let url = format!("https://{registry}/buf.registry.module.v1.DownloadService/Download");
    let request = serde_json::json!({ "values": [{ "resourceRef": { "id": module.commit } }] });
    let response: DownloadResponse = ureq::post(&url)
        .header("Content-Type", "application/json")
        .send_json(&request)
        .with_context(|| format!("downloading {}@{}", module.name, module.commit))?
        .body_mut()
        .with_config()
        .limit(256 << 20)
        .read_json()
        .with_context(|| format!("reading {}@{}", module.name, module.commit))?;
    let [content] = <[Content; 1]>::try_from(response.contents).map_err(|_| {
        anyhow::anyhow!(
            "{}: the registry answered with other than one module",
            module.name
        )
    })?;
    ensure!(
        content.commit.id == module.commit,
        "{}: the registry answered with commit {}, not {}",
        module.name,
        content.commit.id,
        module.commit
    );
    let mut files = BTreeMap::new();
    for file in content.files {
        let data = STANDARD
            .decode(&file.content)
            .with_context(|| format!("{}: {} isn't base64", module.name, file.path))?;
        files.insert(file.path, data);
    }
    let digest = b5(&files);
    ensure!(
        module.digest == digest,
        "{}@{}: the downloaded files have the digest {digest}, but buf.lock pins {}",
        module.name,
        module.commit,
        module.digest
    );
    Ok(files)
}

/// The b5 digest of a module without dependencies: the SHAKE256 of the
/// digest of its manifest, which lists the SHAKE256 of each file, sorted
/// by path.
fn b5(files: &BTreeMap<String, Vec<u8>>) -> String {
    let manifest: String = files
        .iter()
        .map(|(path, data)| format!("shake256:{}  {path}\n", shake256_hex(data)))
        .collect();
    let files_digest = format!("shake256:{}", shake256_hex(manifest.as_bytes()));
    format!("b5:{}", shake256_hex(files_digest.as_bytes()))
}

fn shake256_hex(data: &[u8]) -> String {
    let mut hasher = Shake256::default();
    hasher.update(data);
    let mut out = [0u8; 64];
    hasher.finalize_xof().read(&mut out);
    out.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_version_2_lock() {
        let lock = "# Generated by buf. DO NOT EDIT.\nversion: v2\ndeps:\n  - name: buf.build/googleapis/googleapis\n    commit: 8ac61b9e502343289c8c42b17aa5b245\n    digest: b5:0318\n";
        let modules = parse_lock(lock).unwrap();
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].name, "buf.build/googleapis/googleapis");
        assert_eq!(modules[0].commit, "8ac61b9e502343289c8c42b17aa5b245");
        assert_eq!(modules[0].digest, "b5:0318");
    }

    #[test]
    fn b5_depends_on_every_file_and_path() {
        let mut files = BTreeMap::new();
        files.insert("a.proto".to_owned(), b"syntax = \"proto3\";".to_vec());
        let one = b5(&files);
        files.insert("b.proto".to_owned(), Vec::new());
        let two = b5(&files);
        assert!(one.starts_with("b5:") && one.len() == 3 + 128);
        assert_ne!(one, two);
    }
}

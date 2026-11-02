//! What a new service's crate says it is, taken from the API definitions,
//! so that its scaffolded crate is ready to publish: the crate's
//! description and keywords, its documentation's first line, and its row in
//! README.md's table.
//!
//! What the service serves is, the first that the definitions have:
//!
//! 1. the first sentence of its package's comment, the leading comment of a
//!    file's `package` statement;
//! 2. the first sentence of its service's comment, when it has one service;
//! 3. the resources it serves, but those within another of its resources
//!    (a rule within its ACL): "instances, images and key pairs", spelled as
//!    its comments spell them ("VPCs", "DNS policies").

use std::sync::LazyLock;

use prost_reflect::{DescriptorPool, FileDescriptor, Value};
use regex::Regex;

use crate::clients::message_extension;

static LINTER_DIRECTIVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\(--.*?--\)").expect("a valid pattern"));
static BACKTICKED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`[^`]*`").expect("a valid pattern"));

// The field number of a file's package, as source locations use it.
const FILE_PACKAGE: i32 = 2;

/// A service, as its crate describes it.
#[derive(Debug, PartialEq, Eq)]
pub struct Description {
    /// The service, as its packages name it: `compute`.
    pub service: String,
    /// What it serves, a phrase starting in lowercase: "instances, images
    /// and key pairs". Empty when the definitions say nothing.
    pub what: String,
    /// Its versions: `v1beta1`.
    pub versions: Vec<String>,
}

impl Description {
    /// The crate's description: "The Origin Cloud compute API: instances,
    /// images and key pairs."
    pub fn summary(&self) -> String {
        let name = self.name();
        if self.what.is_empty() {
            format!("The Origin Cloud {name} API.")
        } else {
            format!("The Origin Cloud {name} API: {}.", self.what)
        }
    }

    /// The service's name in prose: its packages', but in capitals when
    /// it is three letters at most, an abbreviation such as IAM.
    fn name(&self) -> String {
        if self.service.len() <= 3 {
            self.service.to_uppercase()
        } else {
            self.service.clone()
        }
    }

    /// The crate's keywords, valid for crates.io: at most five, each at
    /// most 20 characters, starting with a letter.
    pub fn keywords(&self) -> Vec<String> {
        let mut keywords: Vec<String> = ["origincloud", "cloud", "sdk", "api"]
            .map(str::to_owned)
            .to_vec();
        if self.service.len() <= 20 && self.service.starts_with(|c: char| c.is_ascii_lowercase()) {
            keywords.push(self.service.clone());
        }
        keywords
    }

    /// The crate's row in README.md's table of crates.
    pub fn readme_row(&self) -> String {
        let service = &self.service;
        let modules: Vec<String> = self
            .versions
            .iter()
            .map(|v| format!("`origincloud_{service}::{v}`"))
            .collect();
        let what = if self.what.is_empty() {
            format!("The {} API", self.name())
        } else {
            upper_first(&self.what)
        };
        format!(
            "| [`origincloud-{service}`](crates/{service}) | {what} ({}) |",
            modules.join(", ")
        )
    }
}

/// The description of `service`, from the files of its packages,
/// `origincloud.{service}.{version}`, in pool.
pub fn describe(pool: &DescriptorPool, service: &str) -> Description {
    let prefix = format!("origincloud.{service}.");
    let files: Vec<FileDescriptor> = pool
        .files()
        .filter(|f| f.package_name().starts_with(&prefix))
        .collect();
    let mut versions: Vec<String> = files
        .iter()
        .map(|f| f.package_name()[prefix.len()..].to_owned())
        .collect();
    versions.sort();
    versions.dedup();

    let package_comment = files
        .iter()
        .find_map(|f| leading(f, &[FILE_PACKAGE]))
        .map(|c| first_sentence(&c));
    let services: Vec<_> = files.iter().flat_map(FileDescriptor::services).collect();
    let service_comment = match services.as_slice() {
        [only] => leading(&only.parent_file(), only.path()).map(|c| first_sentence(&c)),
        _ => None,
    };
    let what = package_comment
        .filter(|s| !s.is_empty())
        .or(service_comment.filter(|s| !s.is_empty()))
        .map(|s| lower_first(&s))
        .unwrap_or_else(|| resources(pool, &files));
    Description {
        service: service.to_owned(),
        what,
        versions,
    }
}

/// The resources the files serve, but those within another of them, listed
/// as a phrase: "instances, images and key pairs".
fn resources(pool: &DescriptorPool, files: &[FileDescriptor]) -> String {
    // Each resource's first pattern and its plural, in the files' order.
    let mut found: Vec<(String, String)> = Vec::new();
    for file in files {
        for message in file.messages() {
            let Some(resource) = message_extension(pool, &message.options(), "google.api.resource")
            else {
                continue;
            };
            let Some(Value::List(patterns)) = resource
                .get_field_by_name("pattern")
                .map(|v| v.into_owned())
            else {
                continue;
            };
            let Some(pattern) = patterns.first().and_then(Value::as_str).map(str::to_owned) else {
                continue;
            };
            let plural = resource
                .get_field_by_name("plural")
                .and_then(|v| v.as_str().map(str::to_owned))
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| lower_first(&format!("{}s", message.name())));
            found.push((pattern, plural));
        }
    }
    let comments = comments_text(files);
    let mut words: Vec<String> = Vec::new();
    for (pattern, plural) in &found {
        let parent = parent_pattern(pattern);
        if found.iter().any(|(other, _)| *other == parent) {
            continue;
        }
        let phrase = spelled(&comments, &humanize(plural));
        if !words.contains(&phrase) {
            words.push(phrase);
        }
    }
    match words.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The pattern of a resource's parent: `projects/{project}` of
/// `projects/{project}/keyPairs/{key_pair}`, and of a singleton's
/// `projects/{project}/quota`.
fn parent_pattern(pattern: &str) -> String {
    let segments: Vec<&str> = pattern.split('/').collect();
    let own = if segments.last().is_some_and(|s| s.starts_with('{')) {
        2
    } else {
        1
    };
    segments[..segments.len().saturating_sub(own)].join("/")
}

/// A resource's plural as words: `keyPairs` is "key pairs".
fn humanize(plural: &str) -> String {
    let mut out = String::with_capacity(plural.len() + 4);
    for c in plural.chars() {
        if c.is_ascii_uppercase() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// words as the comments spell them, such as "ACLs" for "acls" and "DNS
/// policies" for "dns policies", but for a capital that only starts a
/// sentence: "Floating IPs" is "floating IPs". The first spelling with a
/// capital left counts; words the comments have only in lowercase, or not
/// at all, stay as they are.
fn spelled(comments: &str, words: &str) -> String {
    let parts: Vec<String> = words.split(' ').map(regex::escape).collect();
    let pattern = format!(r"(?i)\b{}\b", parts.join(r"\s+"));
    let Ok(re) = Regex::new(&pattern) else {
        return words.to_owned();
    };
    re.find_iter(comments)
        .map(|m| {
            m.as_str()
                .split_whitespace()
                .map(lower_first)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .find(|spelling| spelling.chars().any(char::is_uppercase))
        .unwrap_or_else(|| words.to_owned())
}

/// Every leading comment of the files, as prose: without linter
/// directives, and without what is in backticks, such as resource names
/// (`projects/{project}/vpcs/{vpc}`) and enum values.
fn comments_text(files: &[FileDescriptor]) -> String {
    let mut text = String::new();
    for file in files {
        if let Some(info) = &file.file_descriptor_proto().source_code_info {
            for location in &info.location {
                if let Some(comment) = &location.leading_comments {
                    let prose = LINTER_DIRECTIVE.replace_all(comment, "");
                    text.push_str(&BACKTICKED.replace_all(&prose, ""));
                    text.push('\n');
                }
            }
        }
    }
    text
}

/// The leading comment at path in file, if it has one.
fn leading(file: &FileDescriptor, path: &[i32]) -> Option<String> {
    let info = file.file_descriptor_proto().source_code_info.as_ref()?;
    info.location
        .iter()
        .find(|l| l.path == path)
        .and_then(|l| l.leading_comments.clone())
        .filter(|c| !c.trim().is_empty())
}

/// A comment as plain text: without linter directives and backticks.
fn plain(comment: &str) -> String {
    LINTER_DIRECTIVE.replace_all(comment, "").replace('`', "")
}

/// The first sentence of a comment's first paragraph, as plain text on one
/// line, without its full stop.
fn first_sentence(comment: &str) -> String {
    let text = plain(comment);
    let paragraph = text
        .split("\n\n")
        .find(|p| !p.trim().is_empty())
        .unwrap_or_default();
    let line = paragraph.split_whitespace().collect::<Vec<_>>().join(" ");
    let sentence = match line.find(". ") {
        Some(end) => &line[..end],
        None => &line,
    };
    sentence.trim_end_matches('.').trim().to_owned()
}

/// text with a lowercase first letter, unless the word it starts is an
/// abbreviation: "The" is "the", but "VPCs" and "IAM" stay.
fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(first), Some(second)) if first.is_uppercase() && second.is_lowercase() => {
            first.to_lowercase().chain(text.chars().skip(1)).collect()
        }
        _ => text.to_owned(),
    }
}

/// text with an uppercase first letter.
fn upper_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use protox::file::{ChainFileResolver, File, FileResolver, GoogleFileResolver};

    const RESOURCE: &str = r#"
syntax = "proto3";
package google.api;
import "google/protobuf/descriptor.proto";
extend google.protobuf.MessageOptions { ResourceDescriptor resource = 1053; }
message ResourceDescriptor {
  string type = 1;
  repeated string pattern = 2;
  string plural = 5;
  string singular = 6;
}
"#;

    struct Sources(Vec<(String, String)>);

    impl FileResolver for Sources {
        fn open_file(&self, name: &str) -> Result<File, protox::Error> {
            match self.0.iter().find(|(n, _)| n == name) {
                Some((_, source)) => File::from_source(name, source),
                None => Err(protox::Error::file_not_found(name)),
            }
        }
    }

    /// A pool of the files, with google/api/resource.proto.
    fn pool(files: &[(&str, &str)]) -> DescriptorPool {
        let mut sources = vec![("google/api/resource.proto".to_owned(), RESOURCE.to_owned())];
        sources.extend(
            files
                .iter()
                .map(|(n, s)| ((*n).to_owned(), (*s).to_owned())),
        );
        let mut resolver = ChainFileResolver::new();
        resolver.add(Sources(sources));
        resolver.add(GoogleFileResolver::new());
        let mut compiler = protox::Compiler::with_file_resolver(resolver);
        compiler.include_imports(true).include_source_info(true);
        let names: Vec<&str> = files.iter().map(|(n, _)| *n).collect();
        compiler.open_files(names).unwrap();
        compiler.descriptor_pool()
    }

    const NETWORK: &str = r#"
syntax = "proto3";
package origincloud.network.v1beta1;
import "google/api/resource.proto";

// VPCs: private networks in a region, named
// `projects/{project}/regions/{region}/vpcs/{vpc}`; a vpcs list.
service VpcService {}

// ACLs: stateless filters at the edge of a subnet. Floating IPs are
// elsewhere.
service AclService {}

message Vpc {
  option (google.api.resource) = { pattern: "projects/{project}/regions/{region}/vpcs/{vpc}" plural: "vpcs" };
}
message Acl {
  option (google.api.resource) = { pattern: "projects/{project}/regions/{region}/acls/{acl}" plural: "acls" };
}
message AclRule {
  option (google.api.resource) = { pattern: "projects/{project}/regions/{region}/acls/{acl}/rules/{rule}" plural: "aclRules" };
}
message FloatingIp {
  option (google.api.resource) = { pattern: "projects/{project}/zones/{zone}/floatingIps/{floating_ip}" plural: "floatingIps" };
}
message KeyPair {
  option (google.api.resource) = { pattern: "projects/{project}/keyPairs/{key_pair}" plural: "keyPairs" };
}
message Quota {
  option (google.api.resource) = { pattern: "projects/{project}/zones/{zone}/floatingIps/{floating_ip}/quota" plural: "quotas" };
}
"#;

    /// A package of several services lists its resources, but those
    /// within another of them, spelled as its comments spell them.
    #[test]
    fn several_services_are_described_by_their_resources() {
        let pool = pool(&[("origincloud/network/v1beta1/network.proto", NETWORK)]);
        let d = describe(&pool, "network");
        assert_eq!(d.what, "VPCs, ACLs, floating IPs and key pairs");
        assert_eq!(d.versions, ["v1beta1"]);
        assert_eq!(
            d.summary(),
            "The Origin Cloud network API: VPCs, ACLs, floating IPs and key pairs."
        );
        assert_eq!(
            d.keywords(),
            ["origincloud", "cloud", "sdk", "api", "network"]
        );
        assert_eq!(
            d.readme_row(),
            "| [`origincloud-network`](crates/network) | VPCs, ACLs, floating IPs and key pairs (`origincloud_network::v1beta1`) |"
        );
    }

    /// One service is described by its comment's first sentence.
    #[test]
    fn one_service_is_described_by_its_comment() {
        let pool = pool(&[(
            "origincloud/location/v1/location.proto",
            "syntax = \"proto3\";\npackage origincloud.location.v1;\n\n// The platform's regions and zones, and which services each zone\n// offers. A zonal list covers one zone.\n//\n// More.\nservice LocationService {}\n",
        )]);
        assert_eq!(
            describe(&pool, "location").summary(),
            "The Origin Cloud location API: the platform's regions and zones, and which services each zone offers."
        );
    }

    /// The package's comment comes first, without linter directives or
    /// backticks.
    #[test]
    fn the_package_comment_comes_first() {
        let pool = pool(&[
            (
                "origincloud/iam/v1beta1/a.proto",
                "syntax = \"proto3\";\n\n// Who may do what (-- api-linter: x=disabled --) in `organizations`. More.\npackage origincloud.iam.v1beta1;\n\n// Roles.\nservice RoleService {}\n",
            ),
            (
                "origincloud/iam/v1beta1/b.proto",
                "syntax = \"proto3\";\npackage origincloud.iam.v1beta1;\n\n// Users.\nservice UserService {}\n",
            ),
        ]);
        assert_eq!(
            describe(&pool, "iam").summary(),
            "The Origin Cloud IAM API: who may do what in organizations."
        );
    }

    #[test]
    fn a_package_without_comments_or_resources_names_only_itself() {
        let pool = pool(&[(
            "origincloud/empty/v1/empty.proto",
            "syntax = \"proto3\";\npackage origincloud.empty.v1;\nservice AService {}\nservice BService {}\n",
        )]);
        let d = describe(&pool, "empty");
        assert_eq!(d.summary(), "The Origin Cloud empty API.");
        assert_eq!(
            d.readme_row(),
            "| [`origincloud-empty`](crates/empty) | The empty API (`origincloud_empty::v1`) |"
        );
    }

    #[test]
    fn a_long_service_name_isnt_a_keyword() {
        let d = Description {
            service: "averyveryverylongservicename".to_owned(),
            what: String::new(),
            versions: Vec::new(),
        };
        assert_eq!(d.keywords(), ["origincloud", "cloud", "sdk", "api"]);
    }

    #[test]
    fn helpers() {
        assert_eq!(humanize("keyPairs"), "key pairs");
        assert_eq!(
            parent_pattern("projects/{project}/keyPairs/{key_pair}"),
            "projects/{project}"
        );
        assert_eq!(
            parent_pattern("projects/{project}/quota"),
            "projects/{project}"
        );
        assert_eq!(parent_pattern("roles/{role}"), "");
        assert_eq!(lower_first("The"), "the");
        assert_eq!(lower_first("IAM roles"), "IAM roles");
        assert_eq!(first_sentence("A. B.\n\nC."), "A");
        assert_eq!(first_sentence("Ends here."), "Ends here");
    }
}

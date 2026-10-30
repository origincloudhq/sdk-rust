//! Doc comments: the proto comments, made into Rust documentation.
//!
//! The comments are cleaned in the descriptors themselves, before any code
//! is generated from them, so that the messages prost-build writes and the
//! clients this generator writes say the same:
//!
//! - the API linter's directives, `(-- ... --)`, are removed;
//! - an enum value in backticks, such as `AVAILABLE`, becomes the Rust
//!   variant it is, such as `volume::State::Available`. Field names stay:
//!   a proto field's Rust name is the same snake case.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::LazyLock;

use prost_reflect::{DescriptorPool, Kind, MessageDescriptor, Value};
use prost_types::FileDescriptorSet;
use regex::Regex;

use crate::names;

static LINTER_DIRECTIVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\(--.*?--\)").expect("a valid pattern"));
static BACKTICKED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`([A-Z][A-Z0-9_]*)`").expect("a valid pattern"));

// Field numbers of the descriptor protos, as source locations use them.
const FILE_MESSAGE_TYPE: i32 = 4;
const FILE_ENUM_TYPE: i32 = 5;
const FILE_SERVICE: i32 = 6;
const MESSAGE_NESTED_TYPE: i32 = 3;
const SERVICE_METHOD: i32 = 2;

/// An enum value a comment may name: its enum's full name and the value.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct EnumValue {
    enum_full_name: String,
    enum_name: String,
}

/// The cleaned comments of the files a package is generated from, by file
/// and location path.
#[derive(Default)]
pub struct Comments {
    leading: HashMap<(String, Vec<i32>), String>,
}

impl Comments {
    /// The leading comment at path in file, cleaned; empty when there is
    /// none.
    pub fn leading(&self, file: &str, path: &[i32]) -> &str {
        self.leading
            .get(&(file.to_owned(), path.to_vec()))
            .map_or("", String::as_str)
    }
}

/// Cleans the comments of every file of `packages` in fds, and returns
/// them.
pub fn clean(
    fds: &mut FileDescriptorSet,
    pool: &DescriptorPool,
    packages: &BTreeSet<String>,
) -> Comments {
    let mut comments = Comments::default();
    for file in &mut fds.file {
        let package = file.package().to_owned();
        if !packages.contains(&package) {
            continue;
        }
        let values = package_enum_values(pool, &package);
        let name = file.name().to_owned();
        let Some(info) = file.source_code_info.as_mut() else {
            continue;
        };
        for location in &mut info.location {
            location.leading_detached_comments.clear();
            let scope = scope_of(pool, &name, &location.path);
            for comment in [
                &mut location.leading_comments,
                &mut location.trailing_comments,
            ] {
                if let Some(text) = comment.as_mut() {
                    *text = clean_text(text, &package, &values, &scope);
                    if text.trim().is_empty() {
                        *comment = None;
                    }
                }
            }
            if let Some(text) = &location.leading_comments {
                comments
                    .leading
                    .insert((name.clone(), location.path.clone()), text.clone());
            }
        }
    }
    comments
}

/// A comment without linter directives, its enum values named as in Rust,
/// and without trailing blank lines.
fn clean_text(
    text: &str,
    package: &str,
    values: &BTreeMap<String, BTreeSet<EnumValue>>,
    scope: &Scope,
) -> String {
    let text = LINTER_DIRECTIVE.replace_all(text, "");
    let text = BACKTICKED.replace_all(&text, |caps: &regex::Captures<'_>| {
        let value = &caps[1];
        match resolve(value, values, scope) {
            Some(found) => format!(
                "`{}::{}`",
                names::relative_type_path(package, &found.enum_full_name),
                names::enum_variant(&found.enum_name, value)
            ),
            None => caps[0].to_owned(),
        }
    });
    let mut lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let mut cleaned = lines.join("\n");
    cleaned.push('\n');
    cleaned
}

/// The enum a value names: the one enum of the closest part of the
/// comment's scope that has it, or else the one enum of the package that
/// has it. Two enums with it in the same part leave it unresolved.
fn resolve<'a>(
    value: &str,
    values: &'a BTreeMap<String, BTreeSet<EnumValue>>,
    scope: &Scope,
) -> Option<&'a EnumValue> {
    let candidates = values.get(value)?;
    for part in scope {
        let in_part: Vec<&EnumValue> = candidates
            .iter()
            .filter(|c| part.contains(&c.enum_full_name))
            .collect();
        match in_part.as_slice() {
            [] => continue,
            [one] => return Some(one),
            _ => return None,
        }
    }
    match candidates.len() {
        1 => candidates.iter().next(),
        _ => None,
    }
}

/// The enums a comment most likely means, in two parts, closest first.
type Scope = [BTreeSet<String>; 2];

/// Every enum value of a package, by its name.
fn package_enum_values(
    pool: &DescriptorPool,
    package: &str,
) -> BTreeMap<String, BTreeSet<EnumValue>> {
    let mut values: BTreeMap<String, BTreeSet<EnumValue>> = BTreeMap::new();
    for e in pool.all_enums().filter(|e| e.package_name() == package) {
        for v in e.values() {
            values
                .entry(v.name().to_owned())
                .or_default()
                .insert(EnumValue {
                    enum_full_name: e.full_name().to_owned(),
                    enum_name: e.name().to_owned(),
                });
        }
    }
    values
}

/// The enums a comment at path most likely means, closest first: those of
/// the message it is in and of that message's fields, or of a method's
/// request and result; then those of the resources these refer to.
fn scope_of(pool: &DescriptorPool, file: &str, path: &[i32]) -> Scope {
    let mut scope: Scope = [BTreeSet::new(), BTreeSet::new()];
    let Some(file) = pool.get_file_by_name(file) else {
        return scope;
    };
    match path {
        [FILE_MESSAGE_TYPE, index, rest @ ..] => {
            let Some(mut message) = file.messages().nth(*index as usize) else {
                return scope;
            };
            let mut rest = rest;
            while let [MESSAGE_NESTED_TYPE, nested, tail @ ..] = rest {
                let child = message.child_messages().nth(*nested as usize);
                match child {
                    Some(child) => message = child,
                    None => break,
                }
                rest = tail;
            }
            add_message(&mut scope[0], &message, true);
            for resource in referenced_resources(pool, &message) {
                add_message(&mut scope[1], &resource, false);
            }
        }
        [FILE_ENUM_TYPE, index, ..] => {
            if let Some(e) = file.enums().nth(*index as usize) {
                scope[0].insert(e.full_name().to_owned());
            }
        }
        [FILE_SERVICE, service, SERVICE_METHOD, method, ..] => {
            let Some(method) = file
                .services()
                .nth(*service as usize)
                .and_then(|s| s.methods().nth(*method as usize))
            else {
                return scope;
            };
            add_message(&mut scope[0], &method.input(), false);
            add_message(&mut scope[0], &method.output(), false);
            if let Some(result) = crate::clients::operation_result(pool, &method) {
                add_message(&mut scope[0], &result, false);
            }
            for resource in referenced_resources(pool, &method.input()) {
                add_message(&mut scope[1], &resource, false);
            }
        }
        _ => {}
    }
    scope
}

/// The resources a request's fields refer to, by their
/// `google.api.resource_reference`: the volume a `DeleteVolumeRequest`
/// names, or that a `ListVolumesRequest` lists.
fn referenced_resources(
    pool: &DescriptorPool,
    request: &MessageDescriptor,
) -> Vec<MessageDescriptor> {
    let (Some(reference), Some(resource)) = (
        pool.get_extension_by_name("google.api.resource_reference"),
        pool.get_extension_by_name("google.api.resource"),
    ) else {
        return Vec::new();
    };
    let mut types = BTreeSet::new();
    for field in request.fields() {
        let options = field.options();
        if !options.has_extension(&reference) {
            continue;
        }
        if let Value::Message(r) = options.get_extension(&reference).into_owned() {
            for name in ["type", "child_type"] {
                if let Some(t) = r
                    .get_field_by_name(name)
                    .and_then(|v| v.as_str().map(str::to_owned))
                {
                    types.insert(t);
                }
            }
        }
    }
    pool.all_messages()
        .filter(|m| {
            let options = m.options();
            options.has_extension(&resource)
                && match options.get_extension(&resource).into_owned() {
                    Value::Message(r) => r
                        .get_field_by_name("type")
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .is_some_and(|t| types.contains(&t)),
                    _ => false,
                }
        })
        .collect()
}

fn add_message(scope: &mut BTreeSet<String>, message: &MessageDescriptor, with_parents: bool) {
    let mut current = Some(message.clone());
    while let Some(m) = current {
        scope.extend(m.child_enums().map(|e| e.full_name().to_owned()));
        for field in m.fields() {
            if let Kind::Enum(e) = field.kind() {
                scope.insert(e.full_name().to_owned());
            }
        }
        current = if with_parents {
            m.parent_message()
        } else {
            None
        };
    }
}

/// A cleaned comment as `#[doc]` attributes, made safe for rustdoc the way
/// prost-build makes the messages' comments safe.
pub fn attributes(comment: &str) -> String {
    let lines: Vec<String> = comment.lines().map(str::to_owned).collect();
    let comments = prost_build::Comments {
        leading: lines,
        ..prost_build::Comments::default()
    };
    let mut rendered = String::new();
    comments.append_with_indent(0, &mut rendered);
    rendered
        .lines()
        .map(|line| format!("#[doc = {:?}]\n", line.strip_prefix("///").unwrap_or(line)))
        .collect()
}

/// A line of documentation the generator writes itself.
pub fn line(text: &str) -> String {
    if text.is_empty() {
        "#[doc = \"\"]\n".to_owned()
    } else {
        format!("#[doc = {:?}]\n", format!(" {text}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values() -> BTreeMap<String, BTreeSet<EnumValue>> {
        let mut values: BTreeMap<String, BTreeSet<EnumValue>> = BTreeMap::new();
        for (e, v) in [
            ("p.Volume.State", "AVAILABLE"),
            ("p.Volume.State", "ERROR"),
            ("p.Snapshot.State", "ERROR"),
            ("p.Volume.ResizeState", "RESIZE_FAILED"),
        ] {
            values.entry(v.to_owned()).or_default().insert(EnumValue {
                enum_full_name: e.to_owned(),
                enum_name: e.rsplit('.').next().unwrap().to_owned(),
            });
        }
        values
    }

    #[test]
    fn enum_values_are_named_as_in_rust() {
        let scope = [
            BTreeSet::from(["p.Volume.State".to_owned()]),
            BTreeSet::new(),
        ];
        let text = " Done once it is `AVAILABLE`, or `ERROR`; `size_gib` and `RESIZE_FAILED`.\n";
        assert_eq!(
            clean_text(text, "p", &values(), &scope),
            " Done once it is `volume::State::Available`, or `volume::State::Error`; `size_gib` and `volume::ResizeState::ResizeFailed`.\n"
        );
    }

    #[test]
    fn an_ambiguous_value_stays_as_it_is() {
        let text = " It is `ERROR`.\n";
        assert_eq!(clean_text(text, "p", &values(), &Scope::default()), text);
    }

    #[test]
    fn linter_directives_are_removed() {
        let text = " Returns it.\n\n (-- api-linter: core::0131=disabled\n     aip.dev/not-precedent: why. --)\n";
        assert_eq!(
            clean_text(text, "p", &values(), &Scope::default()),
            " Returns it.\n"
        );
    }

    #[test]
    fn attributes_escape_brackets_and_urls() {
        let rendered = attributes(" See [the docs] at https://example.com.\n");
        assert_eq!(
            rendered,
            "#[doc = \" See \\\\[the docs\\\\] at <https://example.com.>\"]\n"
        );
    }
}

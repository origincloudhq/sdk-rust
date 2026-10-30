//! Rust names of proto elements, as prost-build gives them.

use heck::{ToSnakeCase, ToUpperCamelCase};

/// Rust's keywords, which prost-build escapes as raw identifiers.
const KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "do", "dyn",
    "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl", "in", "let",
    "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref", "return",
    "static", "struct", "trait", "true", "try", "type", "typeof", "unsafe", "unsized", "use",
    "virtual", "where", "while", "yield",
];

fn escape(ident: String) -> String {
    match ident.as_str() {
        // Keywords that can't be raw identifiers.
        "self" | "super" | "crate" | "Self" => ident + "_",
        _ if KEYWORDS.contains(&ident.as_str()) => format!("r#{ident}"),
        _ => ident,
    }
}

/// A field, method or module name: `size_gib`, `get_volume`.
pub fn snake(name: &str) -> String {
    escape(name.to_snake_case())
}

/// A message or enum name: `Volume`, `ResizeState`.
pub fn upper_camel(name: &str) -> String {
    escape(name.to_upper_camel_case())
}

/// An enum value's variant, without the prefix that repeats the enum's
/// name: `STATE_UNSPECIFIED` of `State` is `Unspecified`, while
/// `RESIZE_COMPLETED` of `ResizeState` stays `ResizeCompleted`.
pub fn enum_variant(enum_name: &str, value: &str) -> String {
    let enum_name = enum_name.to_upper_camel_case();
    let variant = value.to_upper_camel_case();
    let stripped = variant
        .strip_prefix(enum_name.as_str())
        .filter(|rest| rest.starts_with(|c: char| c.is_ascii_uppercase()))
        .unwrap_or(&variant);
    escape(stripped.to_owned())
}

/// The path of a type relative to its package's module: `Volume`, or
/// `volume::State` for one nested in a message.
pub fn relative_type_path(package: &str, full_name: &str) -> String {
    let local = full_name
        .strip_prefix(package)
        .and_then(|rest| rest.strip_prefix('.'))
        .unwrap_or(full_name);
    let parts: Vec<&str> = local.split('.').collect();
    let (last, outer) = parts.split_last().expect("a type has a name");
    let mut path: Vec<String> = outer.iter().map(|m| snake(m)).collect();
    path.push(upper_camel(last));
    path.join("::")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_as_prost_build_gives_them() {
        assert_eq!(snake("size_gib"), "size_gib");
        assert_eq!(snake("type"), "r#type");
        assert_eq!(snake("GetVolume"), "get_volume");
        assert_eq!(upper_camel("storage_class"), "StorageClass");
        assert_eq!(enum_variant("State", "STATE_UNSPECIFIED"), "Unspecified");
        assert_eq!(enum_variant("State", "AVAILABLE"), "Available");
        assert_eq!(
            enum_variant("ResizeState", "RESIZE_COMPLETED"),
            "ResizeCompleted"
        );
        assert_eq!(
            enum_variant("ResizeState", "RESIZE_STATE_UNSPECIFIED"),
            "Unspecified"
        );
        assert_eq!(enum_variant("Media", "MEDIA_SSD"), "Ssd");
        assert_eq!(
            relative_type_path(
                "origincloud.pvs.v1beta1",
                "origincloud.pvs.v1beta1.Volume.State"
            ),
            "volume::State"
        );
        assert_eq!(
            relative_type_path("origincloud.pvs.v1beta1", "origincloud.pvs.v1beta1.Volume"),
            "Volume"
        );
    }
}

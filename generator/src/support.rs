//! What the generated messages can refer to. A package whose fields use a
//! type the SDK has no Rust type for is refused with the fields named,
//! instead of being generated into code that doesn't compile.

use anyhow::{Result, bail};
use prost_reflect::{DescriptorPool, FieldDescriptor, Kind};

/// The well-known types the core crate's `protobuf` module holds with
/// their proto3 JSON mapping.
const WELL_KNOWN: &[&str] = &[
    "BoolValue",
    "BytesValue",
    "DoubleValue",
    "Duration",
    "Empty",
    "FieldMask",
    "FloatValue",
    "Int32Value",
    "Int64Value",
    "ListValue",
    "NullValue",
    "StringValue",
    "Struct",
    "Timestamp",
    "UInt32Value",
    "UInt64Value",
    "Value",
];

/// Fails, naming each field, when a message of `package` has a field whose
/// type the generated code can't refer to.
pub fn check_fields(pool: &DescriptorPool, package: &str) -> Result<()> {
    let mut refused = Vec::new();
    for message in pool.all_messages().filter(|m| m.package_name() == package) {
        for field in message.fields() {
            if let Err(why) = check_field(package, &field) {
                refused.push(format!("{}: {why}", field.full_name()));
            }
        }
    }
    if !refused.is_empty() {
        bail!(
            "the generator doesn't support these fields yet; extend it, or the core crate, before generating them:\n  {}",
            refused.join("\n  ")
        );
    }
    Ok(())
}

fn check_field(package: &str, field: &FieldDescriptor) -> Result<(), String> {
    let name = match field.kind() {
        Kind::Message(m) if m.is_map_entry() => {
            // A map's entry is nested in the message; its value is what
            // matters.
            let value = m.map_entry_value_field();
            return check_field(package, &value);
        }
        Kind::Message(m) => m.full_name().to_owned(),
        Kind::Enum(e) => e.full_name().to_owned(),
        _ => return Ok(()),
    };
    supported(package, &name)
}

/// Whether the generated code of `package` can refer to the message or
/// enum `full_name`.
fn supported(package: &str, full_name: &str) -> Result<(), String> {
    if full_name
        .strip_prefix(package)
        .is_some_and(|rest| rest.starts_with('.'))
    {
        return Ok(());
    }
    if full_name.starts_with("origincloud.longrunning.") {
        return Ok(());
    }
    if full_name == "google.protobuf.Any" {
        return Err(
            "google.protobuf.Any isn't supported: the core crate holds an Any only in its JSON form, \
             which has no protobuf encoding for a generated message to embed"
                .to_owned(),
        );
    }
    if let Some(name) = full_name.strip_prefix("google.protobuf.") {
        if WELL_KNOWN.contains(&name) {
            return Ok(());
        }
        return Err(format!(
            "{full_name} isn't among the well-known types the core crate holds"
        ));
    }
    if full_name.starts_with("google.") {
        return Err(format!("{full_name} has no Rust type in the SDK yet"));
    }
    Err(format!(
        "{full_name} is in another package, which {package}'s crate doesn't depend on"
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use protox::file::{ChainFileResolver, GoogleFileResolver};

    use super::*;
    use crate::MemoryResolver;

    /// google.rpc.Status as googleapis defines it, cut to what matters.
    const STATUS: &str = r#"
        syntax = "proto3";
        package google.rpc;
        import "google/protobuf/any.proto";
        message Status {
          int32 code = 1;
          string message = 2;
          repeated google.protobuf.Any details = 3;
        }
    "#;

    const SCRATCH: &str = r#"
        syntax = "proto3";
        package origincloud.scratch.v1;
        import "google/protobuf/any.proto";
        import "google/protobuf/field_mask.proto";
        import "google/protobuf/timestamp.proto";
        import "google/rpc/status.proto";

        message Supported {
          string name = 1;
          google.protobuf.Timestamp create_time = 2;
          google.protobuf.FieldMask mask = 3;
          map<string, Supported> children = 4;
          Kind kind = 5;
          enum Kind { KIND_UNSPECIFIED = 0; }
        }

        message Unsupported {
          google.protobuf.Any detail = 1;
          google.rpc.Status error = 2;
          map<string, google.protobuf.Any> by_name = 3;
          Supported fine = 4;
        }
    "#;

    fn pool(files: &[(&str, &str)]) -> DescriptorPool {
        let sources: BTreeMap<String, String> = files
            .iter()
            .map(|(name, source)| ((*name).to_owned(), (*source).to_owned()))
            .collect();
        let mut resolver = ChainFileResolver::new();
        resolver.add(MemoryResolver(sources));
        resolver.add(GoogleFileResolver::new());
        let mut compiler = protox::Compiler::with_file_resolver(resolver);
        compiler.include_imports(true);
        let names: Vec<&str> = files.iter().map(|(name, _)| *name).collect();
        compiler
            .open_files(names)
            .expect("the scratch protos compile");
        compiler.descriptor_pool()
    }

    #[test]
    fn any_and_google_rpc_fields_are_refused_by_name() {
        let pool = pool(&[
            ("google/rpc/status.proto", STATUS),
            ("origincloud/scratch/v1/scratch.proto", SCRATCH),
        ]);
        let err = check_fields(&pool, "origincloud.scratch.v1")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(
                "origincloud.scratch.v1.Unsupported.detail: google.protobuf.Any isn't supported"
            ),
            "{err}"
        );
        assert!(
            err.contains(
                "origincloud.scratch.v1.Unsupported.error: google.rpc.Status has no Rust type"
            ),
            "{err}"
        );
        assert!(
            err.contains("origincloud.scratch.v1.Unsupported.by_name: google.protobuf.Any"),
            "{err}"
        );
        assert!(!err.contains("Supported."), "{err}");
        assert!(!err.contains(".fine"), "{err}");
    }

    #[test]
    fn well_known_types_maps_and_the_package_s_own_types_are_supported() {
        let source = SCRATCH.split("message Unsupported").next().unwrap();
        let pool = pool(&[
            ("google/rpc/status.proto", STATUS),
            ("origincloud/scratch/v1/scratch.proto", source),
        ]);
        check_fields(&pool, "origincloud.scratch.v1").unwrap();
    }

    #[test]
    fn another_service_s_types_are_refused() {
        assert!(supported("origincloud.pvs.v1beta1", "origincloud.compute.v1.Instance").is_err());
        assert!(
            supported(
                "origincloud.pvs.v1beta1",
                "origincloud.longrunning.v1beta1.OperationMetadata"
            )
            .is_ok()
        );
        assert!(supported("origincloud.pvs.v1beta1", "google.type.Money").is_err());
        assert!(supported("origincloud.pvs.v1beta1", "google.protobuf.Type").is_err());
    }
}

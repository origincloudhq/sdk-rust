//! The services that have resources: the domains of the resource types the
//! API declares (`google.api.resource` on a message,
//! `google.api.resource_definition` on a file), such as
//! `pvs.origincloudapis.eu/Volume`'s `pvs`. A full resource name's host is
//! one of theirs, so the core crate's `names` module refuses any other; the
//! list is generated so that a new service's names parse without anyone
//! keeping it.

use std::collections::BTreeSet;

use anyhow::{Context as _, Result, bail};
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};

use crate::clients::message_extension;

/// The domain every service's host is under.
const DOMAIN: &str = "origincloudapis.eu";

/// The short codes of the services owning a resource type declared in a
/// file of the API (`origincloud/`), in order.
pub fn resource_services(pool: &DescriptorPool) -> Result<BTreeSet<String>> {
    let mut types = Vec::new();
    for file in pool
        .files()
        .filter(|f| f.name().starts_with("origincloud/"))
    {
        if let Some(ext) = pool.get_extension_by_name("google.api.resource_definition") {
            let options = file.options();
            if options.has_extension(&ext)
                && let Value::List(definitions) = options.get_extension(&ext).into_owned()
            {
                for definition in definitions {
                    if let Value::Message(definition) = definition {
                        types.push(resource_type(&definition, file.name())?);
                    }
                }
            }
        }
        for message in file.messages() {
            collect(pool, &message, &mut types)?;
        }
    }
    types
        .iter()
        .map(|t| service_of(t).with_context(|| format!("the resource type {t:?}")))
        .collect()
}

fn collect(pool: &DescriptorPool, m: &MessageDescriptor, types: &mut Vec<String>) -> Result<()> {
    if let Some(resource) = message_extension(pool, &m.options(), "google.api.resource") {
        types.push(resource_type(&resource, m.full_name())?);
    }
    for nested in m.child_messages() {
        collect(pool, &nested, types)?;
    }
    Ok(())
}

/// The `type` of a `google.api.ResourceDescriptor`.
fn resource_type(resource: &DynamicMessage, on: &str) -> Result<String> {
    resource
        .get_field_by_name("type")
        .and_then(|v| v.as_str().map(str::to_owned))
        .filter(|t| !t.is_empty())
        .with_context(|| format!("{on} declares a resource without a type"))
}

/// The short code of the service owning `resource_type`,
/// `{service}.origincloudapis.eu/{Kind}`.
fn service_of(resource_type: &str) -> Result<String> {
    let Some((domain, kind)) = resource_type.split_once('/') else {
        bail!("a resource type is {{service}}.{DOMAIN}/{{Kind}}");
    };
    let Some(service) = domain
        .strip_suffix(DOMAIN)
        .and_then(|d| d.strip_suffix('.'))
    else {
        bail!("its domain isn't a service's host, {{service}}.{DOMAIN}");
    };
    if service.is_empty()
        || !service
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        bail!("its service, {service:?}, isn't a short code, [a-z0-9]+");
    }
    if !kind.starts_with(|c: char| c.is_ascii_uppercase())
        || !kind.chars().all(|c| c.is_ascii_alphanumeric())
    {
        bail!("its kind, {kind:?}, isn't UpperCamelCase");
    }
    Ok(service.to_owned())
}

/// The core crate's `names/generated/services.rs`, which its `names`
/// module includes.
pub fn generate(services: &BTreeSet<String>) -> String {
    let list: String = services.iter().map(|s| format!("    {s:?},\n")).collect();
    format!(
        "/// The services that have resources, by their short code, in order: the\n\
         /// domains of the resource types the API declares\n\
         /// (`pvs.origincloudapis.eu/Volume`), and so the hosts a full resource name\n\
         /// may start with, `{{service}}.origincloudapis.eu`. A service without\n\
         /// resources of its own, such as `sts`, names nothing.\n\
         pub const RESOURCE_SERVICES: &[&str] = &[\n{list}];\n"
    )
}

#[cfg(test)]
mod tests {
    use protox::file::{ChainFileResolver, GoogleFileResolver};

    use super::*;
    use crate::MemoryResolver;

    /// google.api.resource and resource_definition as googleapis defines
    /// them, with only the fields read here.
    const RESOURCE: &str = r#"
        syntax = "proto3";
        package google.api;
        import "google/protobuf/descriptor.proto";
        message ResourceDescriptor { string type = 1; repeated string pattern = 2; }
        extend google.protobuf.MessageOptions { ResourceDescriptor resource = 1053; }
        extend google.protobuf.FileOptions { repeated ResourceDescriptor resource_definition = 1053; }
    "#;

    fn pool(source: &str) -> DescriptorPool {
        let mut resolver = ChainFileResolver::new();
        resolver.add(MemoryResolver(
            [
                ("google/api/resource.proto".to_owned(), RESOURCE.to_owned()),
                ("origincloud/scratch.proto".to_owned(), source.to_owned()),
            ]
            .into(),
        ));
        resolver.add(GoogleFileResolver::new());
        let mut compiler = protox::Compiler::with_file_resolver(resolver);
        compiler
            .open_files(["origincloud/scratch.proto"])
            .expect("the scratch protos compile");
        compiler.descriptor_pool()
    }

    #[test]
    fn every_declared_resource_type_names_its_service_once() {
        let pool = pool(
            r#"
            syntax = "proto3";
            package origincloud.scratch.v1;
            import "google/api/resource.proto";
            option (google.api.resource_definition) = { type: "kms.origincloudapis.eu/KeyVersion" };
            message Volume {
              option (google.api.resource) = { type: "pvs.origincloudapis.eu/Volume" };
              message Nested { option (google.api.resource) = { type: "compute.origincloudapis.eu/Image" }; }
            }
            message Snapshot { option (google.api.resource) = { type: "pvs.origincloudapis.eu/Snapshot" }; }
            message Plain {}
            "#,
        );
        let services = resource_services(&pool).unwrap();
        assert_eq!(
            services.iter().map(String::as_str).collect::<Vec<_>>(),
            ["compute", "kms", "pvs"]
        );
        assert!(generate(&services).ends_with(
            "pub const RESOURCE_SERVICES: &[&str] = &[\n    \"compute\",\n    \"kms\",\n    \"pvs\",\n];\n"
        ));
    }

    #[test]
    fn a_type_on_another_domain_is_an_error() {
        for bad in [
            "pvs.example.com/Volume",
            "origincloudapis.eu/Volume",
            "pvs.eu-central1.origincloudapis.eu/Volume",
            "pvs.origincloudapis.eu/volume",
            "pvs.origincloudapis.eu",
        ] {
            let pool = pool(&format!(
                r#"
                syntax = "proto3";
                package origincloud.scratch.v1;
                import "google/api/resource.proto";
                message Volume {{ option (google.api.resource) = {{ type: "{bad}" }}; }}
                "#
            ));
            assert!(resource_services(&pool).is_err(), "{bad}");
        }
    }
}

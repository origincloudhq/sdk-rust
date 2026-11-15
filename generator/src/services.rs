//! The API's resource types and the services that own them.
//!
//! - The services that have resources: the domains of the resource types
//!   the API declares (`google.api.resource` on a message,
//!   `google.api.resource_definition` on a file), such as
//!   `pvs.origincloudapis.eu/Volume`'s `pvs`. A full resource name's host is
//!   one of theirs, so the core crate's `names` module refuses any other;
//!   the list is generated so that a new service's names parse without
//!   anyone keeping it.
//! - The services whose full names a path field takes: those of the type
//!   the field refers to ([`Resources::path_services`]).

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context as _, Result, bail};
use prost_reflect::{DescriptorPool, DynamicMessage, FieldDescriptor, MessageDescriptor, Value};

use crate::clients::message_extension;

/// The domain every service's host is under.
const DOMAIN: &str = "origincloudapis.eu";

/// A path field of every resource service: `type: "*"`.
pub const ANY: &str = "*";

/// The resource types the API declares in its files (`origincloud/`).
pub struct Resources {
    /// Each type's patterns, by type.
    patterns: BTreeMap<String, Vec<String>>,
}

impl Resources {
    /// Every resource type the API declares; an error for a type that isn't
    /// `{service}.origincloudapis.eu/{Kind}`.
    pub fn index(pool: &DescriptorPool) -> Result<Resources> {
        let mut found = Vec::new();
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
                            found.push(descriptor(&definition, file.name())?);
                        }
                    }
                }
            }
            for message in file.messages() {
                collect(pool, &message, &mut found)?;
            }
        }
        let mut patterns: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (resource_type, declared) in found {
            service_of(&resource_type)
                .with_context(|| format!("the resource type {resource_type:?}"))?;
            patterns.entry(resource_type).or_default().extend(declared);
        }
        Ok(Resources { patterns })
    }

    /// The short codes of the services owning a declared resource type, in
    /// order.
    pub fn services(&self) -> BTreeSet<String> {
        self.patterns
            .keys()
            .map(|t| service_of(t).expect("index checked every type"))
            .collect()
    }

    /// The short codes of the services whose full names the path field
    /// `field` (the last of a path variable's fields) takes, in order: the
    /// service of the type the field refers to.
    ///
    /// - `google.api.resource_reference` with `type: "*"`: every resource
    ///   service, [`ANY`].
    /// - With a `type`: that type's service.
    /// - With a `child_type`: the services of the declared types whose
    ///   pattern is a pattern of the child's without its last collection and
    ///   id, its parent (`organizations/{organization}`, resourcemanager's
    ///   Organization). A parent no declared type has, such as
    ///   `projects/{project}/zones/{zone}`, is the child's service's.
    /// - The `name` of a resource, a message with `google.api.resource`
    ///   (an update's resource): that resource's service.
    /// - Otherwise: none, which the client reads as its own service.
    pub fn path_services(
        &self,
        pool: &DescriptorPool,
        field: &FieldDescriptor,
    ) -> Result<Vec<String>> {
        if let Some(reference) = field_extension(pool, field, "google.api.resource_reference") {
            let text = |name: &str| {
                reference
                    .get_field_by_name(name)
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .filter(|t| !t.is_empty())
            };
            if let Some(resource_type) = text("type") {
                if resource_type == ANY {
                    return Ok(vec![ANY.to_owned()]);
                }
                return Ok(vec![service_of(&resource_type).with_context(|| {
                    format!("{} refers to {resource_type:?}", field.full_name())
                })?]);
            }
            if let Some(child) = text("child_type") {
                let child_service = service_of(&child)
                    .with_context(|| format!("{} refers to {child:?}", field.full_name()))?;
                return Ok(self
                    .parent_services(&child)
                    .unwrap_or_else(|| vec![child_service]));
            }
        }
        if field.name() == "name"
            && let Some(resource) = message_extension(
                pool,
                &field.parent_message().options(),
                "google.api.resource",
            )
        {
            let resource_type = descriptor(&resource, field.parent_message().full_name())?.0;
            return Ok(vec![service_of(&resource_type)?]);
        }
        Ok(Vec::new())
    }

    /// The services of the declared parents of `child`, in order; `None`
    /// when no declared type is its parent.
    fn parent_services(&self, child: &str) -> Option<Vec<String>> {
        let parents: BTreeSet<&str> = self
            .patterns
            .get(child)?
            .iter()
            .filter_map(|p| p.rsplitn(3, '/').nth(2))
            .collect();
        let services: BTreeSet<String> = self
            .patterns
            .iter()
            .filter(|(_, patterns)| patterns.iter().any(|p| parents.contains(p.as_str())))
            .map(|(t, _)| service_of(t).expect("index checked every type"))
            .collect();
        (!services.is_empty()).then(|| services.into_iter().collect())
    }
}

fn collect(
    pool: &DescriptorPool,
    m: &MessageDescriptor,
    found: &mut Vec<(String, Vec<String>)>,
) -> Result<()> {
    if let Some(resource) = message_extension(pool, &m.options(), "google.api.resource") {
        found.push(descriptor(&resource, m.full_name())?);
    }
    for nested in m.child_messages() {
        collect(pool, &nested, found)?;
    }
    Ok(())
}

/// The message extension `name` on a field's options.
fn field_extension(
    pool: &DescriptorPool,
    field: &FieldDescriptor,
    name: &str,
) -> Option<DynamicMessage> {
    let ext = pool.get_extension_by_name(name)?;
    let options = field.options();
    if !options.has_extension(&ext) {
        return None;
    }
    match options.get_extension(&ext).into_owned() {
        Value::Message(m) => Some(m),
        _ => None,
    }
}

/// The `type` and `pattern`s of a `google.api.ResourceDescriptor`.
fn descriptor(resource: &DynamicMessage, on: &str) -> Result<(String, Vec<String>)> {
    let resource_type = resource
        .get_field_by_name("type")
        .and_then(|v| v.as_str().map(str::to_owned))
        .filter(|t| !t.is_empty())
        .with_context(|| format!("{on} declares a resource without a type"))?;
    let patterns = match resource.get_field_by_name("pattern").as_deref() {
        Some(Value::List(patterns)) => patterns
            .iter()
            .filter_map(|p| p.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    };
    Ok((resource_type, patterns))
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

    /// google.api.resource, resource_definition and resource_reference as
    /// googleapis defines them, with only the fields read here.
    const RESOURCE: &str = r#"
        syntax = "proto3";
        package google.api;
        import "google/protobuf/descriptor.proto";
        message ResourceDescriptor { string type = 1; repeated string pattern = 2; }
        message ResourceReference { string type = 1; string child_type = 2; }
        extend google.protobuf.MessageOptions { ResourceDescriptor resource = 1053; }
        extend google.protobuf.FileOptions { repeated ResourceDescriptor resource_definition = 1053; }
        extend google.protobuf.FieldOptions { ResourceReference resource_reference = 1055; }
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
        let services = Resources::index(&pool).unwrap().services();
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
            assert!(Resources::index(&pool).is_err(), "{bad}");
        }
    }

    /// A path field takes the full names of the service of the type it
    /// refers to.
    #[test]
    fn a_path_field_takes_the_names_of_the_type_it_refers_to() {
        let pool = pool(
            r#"
            syntax = "proto3";
            package origincloud.scratch.v1;
            import "google/api/resource.proto";
            message Project {
              option (google.api.resource) = { type: "resourcemanager.origincloudapis.eu/Project" pattern: "projects/{project}" };
              string name = 1;
            }
            message Zone {
              option (google.api.resource) = { type: "location.origincloudapis.eu/Zone" pattern: "zones/{zone}" };
            }
            message Volume {
              option (google.api.resource) = { type: "pvs.origincloudapis.eu/Volume" pattern: "projects/{project}/zones/{zone}/volumes/{volume}" };
              string name = 1;
            }
            message Cost {
              option (google.api.resource) = { type: "billing.origincloudapis.eu/Cost" pattern: "projects/{project}/costs/{cost}" };
            }
            message PublicImage {
              option (google.api.resource) = { type: "compute.origincloudapis.eu/PublicImage" pattern: "zones/{zone}/images/{image}" };
            }
            message Request {
              string own = 1 [(google.api.resource_reference) = { type: "pvs.origincloudapis.eu/Volume" }];
              string other = 2 [(google.api.resource_reference) = { type: "resourcemanager.origincloudapis.eu/Project" }];
              string any = 3 [(google.api.resource_reference) = { type: "*" }];
              string costs = 4 [(google.api.resource_reference) = { child_type: "billing.origincloudapis.eu/Cost" }];
              string images = 5 [(google.api.resource_reference) = { child_type: "compute.origincloudapis.eu/PublicImage" }];
              string volumes = 6 [(google.api.resource_reference) = { child_type: "pvs.origincloudapis.eu/Volume" }];
              string plain = 7;
            }
            "#,
        );
        let resources = Resources::index(&pool).unwrap();
        let services = |message: &str, field: &str| {
            let m = pool
                .get_message_by_name(&format!("origincloud.scratch.v1.{message}"))
                .unwrap();
            resources
                .path_services(&pool, &m.get_field_by_name(field).unwrap())
                .unwrap()
        };
        assert_eq!(services("Request", "own"), ["pvs"]);
        assert_eq!(services("Request", "other"), ["resourcemanager"]);
        assert_eq!(services("Request", "any"), [ANY]);
        // A child's parent is the declared type with its parent pattern.
        assert_eq!(services("Request", "costs"), ["resourcemanager"]);
        assert_eq!(services("Request", "images"), ["location"]);
        // projects/{project}/zones/{zone} is no declared type: the child's
        // own service.
        assert_eq!(services("Request", "volumes"), ["pvs"]);
        // A resource's own name.
        assert_eq!(services("Volume", "name"), ["pvs"]);
        assert_eq!(services("Project", "name"), ["resourcemanager"]);
        // Nothing to go by: the client's own service.
        assert!(services("Request", "plain").is_empty());
    }
}

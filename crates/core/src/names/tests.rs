use super::*;

/// Whether an error is the refusal a case expects.
type RefusedBy = fn(&FullNameError) -> bool;

const VOLUME: &str =
    "//pvs.origincloudapis.eu/projects/shop-123456/zones/eu-central1-az1/volumes/vol-RAiXDqjUHLh";
const VOLUME_RELATIVE: &str = "projects/shop-123456/zones/eu-central1-az1/volumes/vol-RAiXDqjUHLh";

#[test]
fn the_services_are_the_api_s_with_resources() {
    assert_eq!(
        RESOURCE_SERVICES,
        [
            "billing",
            "compute",
            "dns",
            "iam",
            "kms",
            "location",
            "network",
            "pvs",
            "resourcemanager",
            "support"
        ]
    );
}

#[test]
fn a_full_name_parses_into_its_service_and_relative_name() {
    let name = FullName::parse(VOLUME).unwrap();
    assert_eq!(name.service(), "pvs");
    assert_eq!(name.host(), "pvs.origincloudapis.eu");
    assert_eq!(name.relative(), VOLUME_RELATIVE);
    assert_eq!(name.as_str(), VOLUME);
    assert_eq!(name.to_string(), VOLUME);
    assert_eq!(VOLUME.parse::<FullName>().unwrap(), name);
}

#[test]
fn full_and_relative_convert_by_concatenation() {
    let name = FullName::from_relative("pvs", VOLUME_RELATIVE).unwrap();
    assert_eq!(name.to_string(), VOLUME);
    assert_eq!(FullName::parse(VOLUME).unwrap(), name);
    for service in RESOURCE_SERVICES {
        let name = FullName::from_relative(service, "projects/p").unwrap();
        assert_eq!(
            name.to_string(),
            format!("//{service}.origincloudapis.eu/projects/p")
        );
        assert_eq!(name.service(), *service);
        assert_eq!(name.relative(), "projects/p");
        assert_eq!(FullName::parse(name.as_str()).unwrap(), name);
    }
}

#[test]
fn the_url_adds_the_scheme_and_the_version() {
    let name = FullName::parse(VOLUME).unwrap();
    assert_eq!(
        name.url("v1beta1").unwrap(),
        format!("https://pvs.origincloudapis.eu/v1beta1/{VOLUME_RELATIVE}")
    );
    assert_eq!(
        name.url("v2").unwrap(),
        format!("https://pvs.origincloudapis.eu/v2/{VOLUME_RELATIVE}")
    );
    for not_a_version in [
        "", "v", "1", "V1", "v0", "v01", "v1gamma1", "v1/", "volumes",
    ] {
        assert_eq!(
            name.url(not_a_version),
            Err(FullNameError::NotAVersion(not_a_version.to_owned())),
            "{not_a_version:?}"
        );
    }
}

#[test]
fn names_the_platform_uses_parse() {
    for name in [
        "//iam.origincloudapis.eu/users/2f0a6c0e-7d0e-4f4b-9b8a-0c6f4e1d2a3b",
        "//iam.origincloudapis.eu/users/me",
        "//iam.origincloudapis.eu/roles/iam.editor",
        "//iam.origincloudapis.eu/roles/pvs.volume_admin",
        "//iam.origincloudapis.eu/permissions/pvs.volume.create",
        "//iam.origincloudapis.eu/organizations/o-1/serviceAccounts/9a1b",
        "//iam.origincloudapis.eu/serviceAgents/compute",
        "//iam.origincloudapis.eu/services/admin",
        "//resourcemanager.origincloudapis.eu/projects/shop-123456",
        "//billing.origincloudapis.eu/organizations/o/billingAccounts/ba/spendLimit",
        "//kms.origincloudapis.eu/projects/p/regions/eu-central1/keys/key-RAiXDqjUHLh/versions/1",
        "//location.origincloudapis.eu/zones/eu-central1-az1",
        "//billing.origincloudapis.eu/legalDocuments/terms/versions/2026-12",
        "//compute.origincloudapis.eu/zones/eu-central1-az1/images/img~1_a.b",
    ] {
        let parsed = FullName::parse(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(parsed.to_string(), name);
    }
}

/// Each refusal, by the variant that refuses it.
#[test]
fn refusals_name_their_reason() {
    use FullNameError as E;
    let cases: &[(&str, RefusedBy)] = &[
        // Not a full name at all.
        ("urn:origincloud:storage:o-1:p-1:volume:vol-1", |e| {
            matches!(e, E::Urn(_))
        }),
        ("URN:origincloud:storage:volume:vol-1", |e| {
            matches!(e, E::Urn(_))
        }),
        ("https://pvs.origincloudapis.eu/projects/p", |e| {
            matches!(e, E::Scheme(_))
        }),
        ("https://pvs.origincloudapis.eu/v1beta1/projects/p", |e| {
            matches!(e, E::Scheme(_))
        }),
        ("pvs://projects/p", |e| matches!(e, E::Scheme(_))),
        (VOLUME_RELATIVE, |e| matches!(e, E::Relative(_))),
        ("/projects/p", |e| matches!(e, E::Relative(_))),
        ("pvs.origincloudapis.eu/projects/p", |e| {
            matches!(e, E::Relative(_))
        }),
        ("vol-RAiXDqjUHLh", |e| matches!(e, E::BareId(_))),
        // The host.
        ("//example.com/projects/p", |e| {
            matches!(e, E::UnknownService { .. })
        }),
        ("//pvs.origincloudapis.com/projects/p", |e| {
            matches!(e, E::UnknownService { .. })
        }),
        ("//PVS.origincloudapis.eu/projects/p", |e| {
            matches!(e, E::UnknownService { .. })
        }),
        ("//pvs.origincloudapis.eu:443/projects/p", |e| {
            matches!(e, E::UnknownService { .. })
        }),
        ("//sts.origincloudapis.eu/projects/p", |e| {
            matches!(e, E::UnknownService { .. })
        }),
        ("//observability.origincloudapis.eu/projects/p", |e| {
            matches!(e, E::UnknownService { .. })
        }),
        ("//graph.origincloudapis.eu/projects/p", |e| {
            matches!(e, E::UnknownService { .. })
        }),
        ("//origincloudapis.eu/projects/p", |e| {
            matches!(e, E::UnknownService { .. })
        }),
        ("///projects/p", |e| matches!(e, E::UnknownService { .. })),
        (
            "//pvs.eu-central1-az1.origincloudapis.eu/projects/p",
            |e| matches!(e, E::NotGlobalHost { service, .. } if service == "pvs"),
        ),
        (
            "//network.eu-central1.origincloudapis.eu/projects/p",
            |e| matches!(e, E::NotGlobalHost { service, .. } if service == "network"),
        ),
        // No relative name.
        ("//pvs.origincloudapis.eu", |e| {
            matches!(e, E::NoRelativeName(_))
        }),
        ("//pvs.origincloudapis.eu/", |e| {
            matches!(e, E::NoRelativeName(_))
        }),
        ("//", |e| matches!(e, E::UnknownService { .. })),
        // A version.
        (
            "//pvs.origincloudapis.eu/v1beta1/projects/p",
            |e| matches!(e, E::Version { version, .. } if version == "v1beta1"),
        ),
        (
            "//pvs.origincloudapis.eu/v1/projects/p",
            |e| matches!(e, E::Version { version, .. } if version == "v1"),
        ),
        // The segments.
        ("//pvs.origincloudapis.eu/projects/p/", |e| {
            matches!(e, E::TrailingSlash(_))
        }),
        ("//pvs.origincloudapis.eu/projects//p", |e| {
            matches!(e, E::EmptySegment(_))
        }),
        ("//pvs.origincloudapis.eu//projects/p", |e| {
            matches!(e, E::EmptySegment(_))
        }),
        (
            "//pvs.origincloudapis.eu/projects/./volumes",
            |e| matches!(e, E::DotSegment { segment, .. } if segment == "."),
        ),
        (
            "//pvs.origincloudapis.eu/projects/p/../q",
            |e| matches!(e, E::DotSegment { segment, .. } if segment == ".."),
        ),
        ("//pvs.origincloudapis.eu/projects/p q", |e| {
            matches!(e, E::InvalidSegment { .. })
        }),
        ("//pvs.origincloudapis.eu/projects/p%2Fq", |e| {
            matches!(e, E::InvalidSegment { .. })
        }),
        ("//pvs.origincloudapis.eu/projects/p?view=full", |e| {
            matches!(e, E::InvalidSegment { .. })
        }),
        ("//pvs.origincloudapis.eu/projects/p#x", |e| {
            matches!(e, E::InvalidSegment { .. })
        }),
        ("//pvs.origincloudapis.eu/projects/p:q", |e| {
            matches!(e, E::InvalidSegment { .. })
        }),
        ("//pvs.origincloudapis.eu/projects/pä", |e| {
            matches!(e, E::InvalidSegment { .. })
        }),
        ("//pvs.origincloudapis.eu/projects/p\n", |e| {
            matches!(e, E::InvalidSegment { .. })
        }),
    ];
    for (name, refused_by) in cases {
        let err = FullName::parse(name).expect_err(name);
        assert!(refused_by(&err), "{name}: refused as {err:?}");
        assert!(
            err.to_string().contains(&format!("{name:?}")),
            "{name}: {err}"
        );
    }
    assert_eq!(FullName::parse(""), Err(FullNameError::Empty));
}

#[test]
fn a_relative_name_is_refused_where_a_full_one_is_expected_and_back() {
    assert!(matches!(
        FullName::parse(VOLUME_RELATIVE),
        Err(FullNameError::Relative(_))
    ));
    assert!(matches!(
        FullName::from_relative("pvs", VOLUME),
        Err(FullNameError::NotRelative(_))
    ));
}

#[test]
fn from_relative_refuses_what_parse_refuses_in_a_relative_name() {
    use FullNameError as E;
    let cases: &[(&str, RefusedBy)] = &[
        ("", |e| matches!(e, E::NoRelativeName(_))),
        ("/projects/p", |e| matches!(e, E::EmptySegment(_))),
        ("projects/p/", |e| matches!(e, E::TrailingSlash(_))),
        ("projects//p", |e| matches!(e, E::EmptySegment(_))),
        ("projects/..", |e| matches!(e, E::DotSegment { .. })),
        ("v1beta1/projects/p", |e| matches!(e, E::Version { .. })),
        ("projects/p q", |e| matches!(e, E::InvalidSegment { .. })),
    ];
    for (relative, refused_by) in cases {
        let err = FullName::from_relative("pvs", relative).expect_err(relative);
        assert!(refused_by(&err), "{relative:?}: refused as {err:?}");
    }
}

#[test]
fn from_relative_refuses_a_service_without_resources() {
    for service in [
        "sts",
        "observability",
        "graph",
        "",
        "PVS",
        "pvs.origincloudapis.eu",
    ] {
        assert!(
            matches!(
                FullName::from_relative(service, "projects/p"),
                Err(FullNameError::UnknownService { .. })
            ),
            "{service:?}"
        );
    }
}

#[test]
fn a_version_is_refused_only_in_the_first_segment() {
    // A key version's id is a number, and an id may look like a version.
    assert!(FullName::parse("//kms.origincloudapis.eu/projects/v1/regions/r/keys/k").is_ok());
    assert!(FullName::parse("//billing.origincloudapis.eu/legalDocuments/v1").is_ok());
}

#[test]
fn equal_names_are_equal_values_and_order_as_their_strings() {
    let mut names: Vec<FullName> = [
        "//pvs.origincloudapis.eu/projects/p/zones/z/volumes/b",
        "//compute.origincloudapis.eu/projects/p/zones/z/instances/a",
        "//pvs.origincloudapis.eu/projects/p/zones/z/volumes/a",
        "//network.origincloudapis.eu/projects/p/regions/r/vpcs/v",
        "//iam.origincloudapis.eu/users/u",
    ]
    .iter()
    .map(|n| FullName::parse(n).unwrap())
    .collect();
    let mut strings: Vec<String> = names.iter().map(ToString::to_string).collect();
    names.sort();
    strings.sort();
    assert_eq!(
        names.iter().map(ToString::to_string).collect::<Vec<_>>(),
        strings
    );
    assert_ne!(
        FullName::from_relative("pvs", "projects/p").unwrap(),
        FullName::from_relative("compute", "projects/p").unwrap()
    );
}

#[test]
fn serde_and_display_both_give_the_full_form() {
    let name = FullName::parse(VOLUME).unwrap();
    let json = serde_json::to_value(&name).unwrap();
    assert_eq!(json, serde_json::Value::String(VOLUME.to_owned()));
    assert_eq!(format!("{name:?}"), format!("FullName({VOLUME:?})"));
    assert_eq!(String::from(name.clone()), VOLUME);
    assert_eq!(serde_json::from_value::<FullName>(json).unwrap(), name);
}

#[test]
fn deserializing_is_as_strict_as_parsing() {
    for refused in [
        VOLUME_RELATIVE,
        "vol-RAiXDqjUHLh",
        "urn:origincloud:storage:o:p:volume:v",
        "//pvs.eu-central1-az1.origincloudapis.eu/projects/p",
        "//pvs.origincloudapis.eu/v1beta1/projects/p",
        "//pvs.origincloudapis.eu/projects/p/",
    ] {
        let err = serde_json::from_value::<FullName>(serde_json::Value::String(refused.to_owned()))
            .expect_err(refused);
        assert_eq!(
            err.to_string(),
            FullName::parse(refused).unwrap_err().to_string(),
            "{refused}"
        );
    }
    assert!(serde_json::from_value::<FullName>(serde_json::json!(7)).is_err());
}

#[test]
fn each_principal_kind_has_its_own_form() {
    for (full, principal) in [
        (
            "//iam.origincloudapis.eu/users/u-1",
            Principal::User { user: "u-1" },
        ),
        (
            "//iam.origincloudapis.eu/organizations/o-1/serviceAccounts/sa-1",
            Principal::ServiceAccount {
                organization: "o-1",
                service_account: "sa-1",
            },
        ),
        (
            "//iam.origincloudapis.eu/serviceAgents/compute",
            Principal::ServiceAgent {
                service_agent: "compute",
            },
        ),
        (
            "//iam.origincloudapis.eu/services/admin",
            Principal::Service { service: "admin" },
        ),
    ] {
        let name = FullName::parse(full).unwrap();
        assert_eq!(name.principal(), Some(principal), "{full}");
        assert_eq!(principal.full_name().unwrap(), name, "{full}");
        assert_eq!(name.role(), None, "{full}");
    }
}

#[test]
fn other_names_are_no_principal() {
    for full in [
        // The organisation principal is not decided.
        "//iam.origincloudapis.eu/organizations/o-1",
        "//resourcemanager.origincloudapis.eu/organizations/o-1",
        // A principal's spelling on another service.
        "//resourcemanager.origincloudapis.eu/users/u-1",
        // A child of a principal.
        "//iam.origincloudapis.eu/organizations/o/serviceAccounts/sa/federatedCredentials/f",
        "//iam.origincloudapis.eu/users/u-1/sessions/s",
        "//iam.origincloudapis.eu/roles/iam.editor",
    ] {
        assert_eq!(FullName::parse(full).unwrap().principal(), None, "{full}");
    }
}

#[test]
fn a_principal_s_part_is_one_segment() {
    for principal in [
        Principal::User { user: "u/1" },
        Principal::ServiceAccount {
            organization: "o",
            service_account: "sa/keys/k",
        },
        Principal::ServiceAgent {
            service_agent: "a/b",
        },
        Principal::Service { service: "" },
    ] {
        assert!(principal.full_name().is_err(), "{principal:?}");
    }
}

#[test]
fn a_role_is_named_with_dots_and_never_slashes() {
    let role = FullName::parse("//iam.origincloudapis.eu/roles/iam.editor").unwrap();
    assert_eq!(role.role(), Some("iam.editor"));
    assert_eq!(role.principal(), None);
    assert_eq!(FullName::of_role("iam.editor").unwrap(), role);
    let slashed = FullName::parse("//iam.origincloudapis.eu/roles/iam/editor").unwrap();
    assert_eq!(slashed.role(), None);
    assert!(FullName::of_role("iam/editor").is_err());
    assert!(FullName::of_role("").is_err());
    let elsewhere =
        FullName::parse("//resourcemanager.origincloudapis.eu/roles/iam.editor").unwrap();
    assert_eq!(elsewhere.role(), None);
}

/// A path field takes a relative name, or a full name of the client's own
/// service, which is sent as its relative name.
#[test]
fn a_path_name_is_the_relative_name_of_the_client_s_own_service() {
    for (value, sent) in [
        (VOLUME, VOLUME_RELATIVE),
        (VOLUME_RELATIVE, VOLUME_RELATIVE),
        // A relative name is checked against the path template, as before.
        ("projects/p 1/zones/z", "projects/p 1/zones/z"),
        ("", ""),
    ] {
        assert_eq!(path_name("pvs", &[], "name", value), Ok(sent), "{value:?}");
    }
}

#[test]
fn a_path_name_of_another_host_or_no_full_name_is_refused() {
    for (value, says) in [
        (
            "//compute.origincloudapis.eu/projects/p/zones/z/instances/i",
            "name must be a name on pvs.origincloudapis.eu",
        ),
        (
            "//resourcemanager.origincloudapis.eu/projects/p",
            "a name on resourcemanager.origincloudapis.eu",
        ),
        (
            "//pvs.eu-central1-az1.origincloudapis.eu/projects/p/zones/z/volumes/v",
            "the service's global host, pvs.origincloudapis.eu",
        ),
        (
            "//pvs.eu-central1.origincloudapis.eu/projects/p/zones/z/volumes/v",
            "the service's global host",
        ),
        (
            "//pvs.origincloudapis.eu/v1beta1/projects/p/zones/z/volumes/v",
            "carries the API version",
        ),
        ("urn:origincloud:storage:o:p:volume:v", "is a URN"),
        (
            "https://pvs.origincloudapis.eu/v1beta1/projects/p/zones/z/volumes/v",
            "is a URL",
        ),
        (
            "//pvs.origincloudapis.eu/projects/p/zones/z/volumes/",
            "ends with a '/'",
        ),
        (
            "//pvs.origincloudapis.eu/projects/p q/zones/z/volumes/v",
            "has the segment",
        ),
        (
            "//example.com/projects/p",
            "is not the host of one of the platform's APIs",
        ),
    ] {
        let err = path_name("pvs", &[], "name", value).expect_err(value);
        assert!(err.starts_with("name"), "{value}: {err}");
        assert!(err.contains(says), "{value}: {err}");
    }
    // A service with no resources takes no full name at all.
    assert!(
        path_name(
            "observability",
            &[],
            "parent",
            "//observability.origincloudapis.eu/projects/p"
        )
        .is_err()
    );
}

/// A path field referring to another service's type takes that service's
/// full names, and no other's.
#[test]
fn a_path_name_is_checked_against_the_type_the_field_refers_to() {
    let project = "//resourcemanager.origincloudapis.eu/projects/p";
    for own in ["billing", "observability"] {
        assert_eq!(
            path_name(own, &["resourcemanager"], "parent", project),
            Ok("projects/p")
        );
        assert_eq!(
            path_name(own, &["resourcemanager"], "parent", "projects/p"),
            Ok("projects/p")
        );
        let err = path_name(
            own,
            &["resourcemanager"],
            "parent",
            "//pvs.origincloudapis.eu/projects/p",
        )
        .unwrap_err();
        assert!(
            err.contains("parent must be a name on resourcemanager.origincloudapis.eu, got"),
            "{err}"
        );
        // The client's own host isn't one the field takes.
        let err = path_name(
            own,
            &["resourcemanager"],
            "parent",
            &format!("//{own}.origincloudapis.eu/projects/p"),
        )
        .unwrap_err();
        // billing's names are no projects; observability names nothing.
        assert!(
            err.starts_with("parent") && err.contains(&format!("{own}.origincloudapis.eu")),
            "{err}"
        );
    }
    // Several services: the error names each host.
    let err = path_name(
        "iam",
        &["compute", "location"],
        "parent",
        "//pvs.origincloudapis.eu/zones/z",
    )
    .unwrap_err();
    assert!(
        err.contains(
            "must be a name on one of compute.origincloudapis.eu, location.origincloudapis.eu"
        ),
        "{err}"
    );
    // Any service: every resource service's full name, still checked.
    for value in [VOLUME, project] {
        assert!(
            path_name("iam", &[ANY_SERVICE], "resource", value).is_ok(),
            "{value}"
        );
    }
    assert!(
        path_name(
            "iam",
            &[ANY_SERVICE],
            "resource",
            "//sts.origincloudapis.eu/x/y"
        )
        .is_err()
    );
}

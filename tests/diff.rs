//! Comparing schema versions, and following references while doing it.

use ash_jsonschema_builder::diff::{ChangeKind, Effect, Role, diff, diff_resolved};
use ash_jsonschema_builder::{Registry, Schema};

/// Two versions of one document, each in its own registry — the shape a real
/// compatibility gate has.
fn versioned(length: u64) -> (Registry, Schema) {
    let schema = Schema::object()
        .id("https://example.com/user")
        .def("Name", Schema::string().min_length(length))
        .required_property("name", Schema::reference("#/$defs/Name"))
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&schema).unwrap();
    (registry, schema)
}

/// A change inside a shared definition is attributed to the place that uses it,
/// which is the point of resolving at all.
#[test]
fn a_change_behind_a_reference_is_reported_at_the_use_site() {
    let (before_registry, before) = versioned(1);
    let (after_registry, after) = versioned(5);

    let report = diff_resolved(&before_registry, &before, &after_registry, &after);

    assert_eq!(report.changes.len(), 1);
    assert_eq!(report.changes[0].location, "/properties/name");
    assert!(matches!(
        report.changes[0].kind,
        ChangeKind::Narrowed { .. }
    ));
    assert!(report.is_breaking_for(Role::Writer));
    assert!(!report.is_breaking_for(Role::Reader));
}

/// The relative form is what almost every schema actually writes, so it has to
/// resolve — against the `$id` of the document the reference sits in.
#[test]
fn a_relative_reference_resolves_against_the_enclosing_id() {
    let (before_registry, before) = versioned(1);
    let (after_registry, after) = versioned(5);

    // `#/$defs/Name` has no meaning without a base; the differ supplies one.
    assert!(before_registry.resolve("#/$defs/Name").is_none());

    let report = diff_resolved(&before_registry, &before, &after_registry, &after);
    assert_eq!(report.changes.len(), 1);
    assert_eq!(report.changes[0].location, "/properties/name");
}

/// Without a registry the reference is opaque, so the change surfaces where the
/// definition sits rather than where it is used.
#[test]
fn an_unresolved_diff_reports_the_definition_instead_of_the_use_site() {
    let (_, before) = versioned(1);
    let (_, after) = versioned(5);

    let report = diff(&before, &after);

    assert_eq!(report.changes.len(), 1);
    assert_eq!(report.changes[0].location, "/$defs/Name");
}

/// Resolving must not report one change twice — once at the definition and
/// again at each use.
#[test]
fn a_resolved_change_is_reported_once_per_use_not_also_at_the_definition() {
    let version = |length: u64| {
        let schema = Schema::object()
            .id("https://example.com/s")
            .def("Name", Schema::string().min_length(length))
            .property("first", Schema::reference("#/$defs/Name"))
            .property("last", Schema::reference("#/$defs/Name"))
            .build()
            .unwrap();
        let mut registry = Registry::new();
        registry.insert(&schema).unwrap();
        (registry, schema)
    };

    let (before_registry, before) = version(1);
    let (after_registry, after) = version(5);

    let report = diff_resolved(&before_registry, &before, &after_registry, &after);

    // Two uses, two reports, and nothing at `/$defs/Name`.
    let locations: Vec<&str> = report
        .changes
        .iter()
        .map(|change| change.location.as_str())
        .collect();
    assert_eq!(locations.len(), 2);
    assert!(locations.contains(&"/properties/first"));
    assert!(locations.contains(&"/properties/last"));
    assert!(!locations.iter().any(|location| location.contains("$defs")));
}

/// A recursive schema must terminate rather than diffing forever.
#[test]
fn a_recursive_schema_terminates() {
    let node = |tagged: bool| {
        let mut builder = Schema::object()
            .id("https://example.com/node")
            .property("value", Schema::string())
            .property("next", Schema::reference("https://example.com/node"));
        if tagged {
            builder = builder.property("tag", Schema::string());
        }
        let schema = builder.build().unwrap();
        let mut registry = Registry::new();
        registry.insert(&schema).unwrap();
        (registry, schema)
    };

    let (before_registry, before) = node(false);
    let (after_registry, after) = node(true);

    let report = diff_resolved(&before_registry, &before, &after_registry, &after);

    assert!(
        report
            .changes
            .iter()
            .any(|change| change.kind == ChangeKind::AddedOptionalProperty("tag".into()))
    );
}

/// Two documents that refer to each other, the cycle a naive differ trips on.
#[test]
fn mutually_recursive_schemas_terminate() {
    let pair = |extra: bool| {
        let a = Schema::object()
            .id("https://example.com/a")
            .property("b", Schema::reference("https://example.com/b"))
            .build()
            .unwrap();
        let mut builder = Schema::object()
            .id("https://example.com/b")
            .property("a", Schema::reference("https://example.com/a"));
        if extra {
            builder = builder.property("note", Schema::string());
        }
        let b = builder.build().unwrap();

        let mut registry = Registry::new();
        registry.insert(&a).unwrap();
        registry.insert(&b).unwrap();
        (registry, a)
    };

    let (before_registry, before) = pair(false);
    let (after_registry, after) = pair(true);

    // Terminating at all is the assertion.
    let report = diff_resolved(&before_registry, &before, &after_registry, &after);
    assert!(!report.is_empty());
}

/// A reference the registry cannot resolve falls back to comparing targets.
#[test]
fn an_unresolvable_reference_falls_back_to_comparing_targets() {
    let before = Schema::object()
        .id("https://example.com/s")
        .property("a", Schema::reference("#/$defs/Missing"))
        .build()
        .unwrap();
    let after = Schema::object()
        .id("https://example.com/s")
        .property("a", Schema::reference("#/$defs/AlsoMissing"))
        .build()
        .unwrap();

    let mut before_registry = Registry::new();
    before_registry.insert(&before).unwrap();
    let mut after_registry = Registry::new();
    after_registry.insert(&after).unwrap();

    let report = diff_resolved(&before_registry, &before, &after_registry, &after);

    assert_eq!(report.changes.len(), 1);
    assert_eq!(report.changes[0].location, "/properties/a");
    assert_eq!(report.changes[0].for_writer, Effect::Unknown);
}

/// A nested `$id` rebases the references beneath it.
#[test]
fn a_nested_id_rebases_references_below_it() {
    let version = |length: u64| {
        let schema = Schema::object()
            .id("https://example.com/root")
            .def(
                "Inner",
                Schema::object()
                    .id("https://example.com/inner")
                    .def("Deep", Schema::string().min_length(length))
                    .property("deep", Schema::reference("#/$defs/Deep")),
            )
            .property("inner", Schema::reference("https://example.com/inner"))
            .build()
            .unwrap();
        let mut registry = Registry::new();
        registry.insert(&schema).unwrap();
        (registry, schema)
    };

    let (before_registry, before) = version(1);
    let (after_registry, after) = version(9);

    let report = diff_resolved(&before_registry, &before, &after_registry, &after);

    // `#/$defs/Deep` resolved against the *inner* `$id`, not the root's.
    assert!(!report.is_empty());
    assert!(
        report
            .changes
            .iter()
            .all(|change| matches!(change.kind, ChangeKind::Narrowed { .. })),
        "expected only the tightened bound, got {report}",
    );
}

/// Positional lists pair by index only when the length is unchanged.
#[test]
fn a_list_of_subschemas_is_compared_positionally_when_its_length_holds() {
    let branches = |length: u64| {
        [
            Schema::from(Schema::string().min_length(length)),
            Schema::from(Schema::integer()),
        ]
    };
    let before = Schema::builder().any_of(branches(1)).build().unwrap();
    let after = Schema::builder().any_of(branches(3)).build().unwrap();

    let report = diff(&before, &after);
    assert_eq!(report.changes.len(), 1);
    assert_eq!(report.changes[0].location, "/anyOf/0");

    // A list that changed length is not paired up, since which branch is which
    // is no longer knowable.
    let shorter = Schema::builder()
        .any_of([Schema::from(Schema::string().min_length(1))])
        .build()
        .unwrap();
    let report = diff(&before, &shorter);
    assert_eq!(report.changes.len(), 1);
    assert_eq!(report.changes[0].location, "");
    assert_eq!(report.changes[0].for_writer, Effect::Unknown);
}

/// A realistic version bump, reported from both sides at once.
#[test]
fn a_version_bump_reports_each_side_separately() {
    let v1 = Schema::object()
        .id("https://example.com/user")
        .required_property("id", Schema::string())
        .property("nickname", Schema::string())
        .property("age", Schema::integer().minimum(0))
        .build()
        .unwrap();

    let v2 = Schema::object()
        .id("https://example.com/user")
        .required_property("id", Schema::string())
        .required_property("email", Schema::string())
        .property("age", Schema::integer().minimum(13))
        .build()
        .unwrap();

    let report = diff(&v1, &v2);

    // Writers: a new required field, and a tighter bound.
    assert_eq!(report.breaking_for(Role::Writer).count(), 2);
    // Readers: a field they may have relied on is gone.
    assert_eq!(report.breaking_for(Role::Reader).count(), 1);

    let text = report.to_string();
    assert!(text.contains("added required property `email`"));
    assert!(text.contains("removed property `nickname`"));
}

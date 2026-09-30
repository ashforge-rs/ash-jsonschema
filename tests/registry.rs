//! Reference resolution: indexing schemas by URI and following `$ref`.

use std::sync::Arc;

use ash_jsonschema::registry::ANONYMOUS;
use ash_jsonschema::{ErrorKind, Registry, RegistryError, Schema};

/// The everyday case: a `$ref` into the document's own `$defs`.
#[test]
fn a_local_reference_resolves_to_its_definition() {
    let schema = Schema::object()
        .id("https://example.com/user")
        .def("Name", Schema::string().min_length(1))
        .required_property("name", Schema::reference("#/$defs/Name"))
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&schema).unwrap();

    let target = registry
        .resolve("https://example.com/user#/$defs/Name")
        .expect("the definition is indexed");
    assert_eq!(target.get("minLength"), Some(&serde_json::json!(1)));

    registry.check_references().unwrap();
}

/// A schema with no `$id` is still usable — its internal references resolve
/// against the anonymous base rather than failing.
#[test]
fn a_schema_without_an_id_still_resolves_its_own_references() {
    let schema = Schema::object()
        .def("Name", Schema::string())
        .property("name", Schema::reference("#/$defs/Name"))
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&schema).unwrap();

    assert!(registry.contains(ANONYMOUS));
    assert!(
        registry
            .resolve(&format!("{ANONYMOUS}#/$defs/Name"))
            .is_some()
    );
    registry.check_references().unwrap();
}

/// An `$anchor` names the same place a pointer does.
#[test]
fn an_anchor_resolves_to_the_subschema_carrying_it() {
    let schema = Schema::object()
        .id("https://example.com/s")
        .def("A", Schema::string().anchor("letter-a"))
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&schema).unwrap();

    let by_pointer = registry.resolve("https://example.com/s#/$defs/A").unwrap();
    let by_anchor = registry.resolve("https://example.com/s#letter-a").unwrap();
    assert_eq!(by_pointer, by_anchor);
}

/// References across two separately inserted documents.
#[test]
fn a_reference_reaches_another_document_in_the_registry() {
    let address = Schema::object()
        .id("https://example.com/address")
        .required_property("city", Schema::string())
        .build()
        .unwrap();

    let user = Schema::object()
        .id("https://example.com/user")
        .property("address", Schema::reference("https://example.com/address"))
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&address).unwrap();
    registry.insert(&user).unwrap();

    assert_eq!(registry.len(), 2);
    registry.check_references().unwrap();

    let target = registry.resolve("https://example.com/address").unwrap();
    assert_eq!(target.get("type"), Some(&serde_json::json!("object")));
}

/// A relative `$ref` resolves against the `$id` of the document it sits in.
#[test]
fn a_relative_reference_resolves_against_its_documents_id() {
    let address = Schema::object()
        .id("https://example.com/schemas/address")
        .build()
        .unwrap();

    // Written as a sibling path, not an absolute URI.
    let user = Schema::object()
        .id("https://example.com/schemas/user")
        .property("address", Schema::reference("address"))
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&address).unwrap();
    registry.insert(&user).unwrap();

    registry.check_references().unwrap();

    let reference = registry
        .references()
        .find(|reference| reference.target == "address")
        .unwrap();
    assert_eq!(reference.absolute(), "https://example.com/schemas/address");
}

/// The check `build()` deliberately does not run.
#[test]
fn a_dangling_reference_is_reported_at_the_keyword_that_wrote_it() {
    let schema = Schema::object()
        .id("https://example.com/s")
        .property("a", Schema::reference("#/$defs/Nope"))
        .build()
        .expect("a dangling ref is still a legal 2020-12 document");

    let mut registry = Registry::new();
    registry.insert(&schema).unwrap();

    let error = registry.check_references().unwrap_err();
    assert_eq!(error.violations().len(), 1);
    assert_eq!(error.first().location, "/properties/a/$ref");
    assert!(error.first().message.contains("#/$defs/Nope"));
}

/// A reference to a document nobody inserted dangles; inserting it fixes that.
#[test]
fn a_reference_to_an_absent_document_dangles_until_it_is_inserted() {
    let user = Schema::object()
        .id("https://example.com/user")
        .property("address", Schema::reference("https://example.com/address"))
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&user).unwrap();
    assert!(registry.check_references().is_err());

    let address = Schema::object()
        .id("https://example.com/address")
        .build()
        .unwrap();
    registry.insert(&address).unwrap();
    registry.check_references().unwrap();
}

/// Every dangling reference is reported, not just the first.
#[test]
fn every_dangling_reference_is_reported() {
    let schema = Schema::object()
        .id("https://example.com/s")
        .property("a", Schema::reference("#/$defs/A"))
        .property("b", Schema::reference("#/$defs/B"))
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&schema).unwrap();

    let error = registry.check_references().unwrap_err();
    assert_eq!(error.violations().len(), 2);
}

/// Two schemas claiming one `$id` would make resolution ambiguous.
#[test]
fn a_duplicate_id_is_refused_but_can_be_replaced_deliberately() {
    let first = Schema::object()
        .id("https://example.com/s")
        .title("first")
        .build()
        .unwrap();
    let second = Schema::object()
        .id("https://example.com/s")
        .title("second")
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&first).unwrap();

    assert_eq!(
        registry.insert(&second),
        Err(RegistryError::DuplicateId {
            id: "https://example.com/s".to_owned()
        }),
    );

    // The first is still the one indexed.
    let held = registry.resolve("https://example.com/s").unwrap();
    assert_eq!(held.get("title"), Some(&serde_json::json!("first")));

    registry.replace(&second).unwrap();
    let held = registry.resolve("https://example.com/s").unwrap();
    assert_eq!(held.get("title"), Some(&serde_json::json!("second")));
    assert_eq!(registry.len(), 1);
}

/// An `$id` a registry cannot use as a base is refused with a reason.
#[test]
fn an_id_that_cannot_be_a_base_is_refused() {
    let relative = Schema::object().id("just-a-name").build().unwrap();
    let mut registry = Registry::new();

    let error = registry.insert(&relative).unwrap_err();
    assert!(matches!(error, RegistryError::UnusableId { .. }));
    assert!(error.to_string().contains("relative"));
}

/// A nested `$id` starts its own resource, and is indexed under the URI it
/// resolves to rather than under the enclosing document.
#[test]
fn a_nested_id_becomes_its_own_resource() {
    let schema = Schema::object()
        .id("https://example.com/root")
        .def(
            "Inner",
            Schema::object()
                .id("https://example.com/inner")
                .def("Deep", Schema::string()),
        )
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&schema).unwrap();

    assert!(registry.contains("https://example.com/root"));
    assert!(registry.contains("https://example.com/inner"));

    // The inner resource's pointers restart at its own root.
    assert!(
        registry
            .resolve("https://example.com/inner#/$defs/Deep")
            .is_some()
    );
}

/// A nested `$id` written relatively resolves against the base enclosing it.
#[test]
fn a_relative_nested_id_resolves_against_its_enclosing_base() {
    let schema = Schema::object()
        .id("https://example.com/schemas/root")
        .def(
            "Inner",
            Schema::object().id("inner").def("Deep", Schema::string()),
        )
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&schema).unwrap();

    assert!(registry.contains("https://example.com/schemas/inner"));
    assert!(
        registry
            .resolve("https://example.com/schemas/inner#/$defs/Deep")
            .is_some()
    );
}

/// A `$ref` cycle is legal and must not hang the resolver.
#[test]
fn a_reference_cycle_resolves_without_hanging() {
    // A linked list: `next` points back at the node schema.
    let node = Schema::object()
        .id("https://example.com/node")
        .property("value", Schema::string())
        .property("next", Schema::reference("https://example.com/node"))
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&node).unwrap();

    registry.check_references().unwrap();

    let target = registry.resolve("https://example.com/node").unwrap();
    assert!(target.get("properties").is_some());
}

/// Two documents referring to each other, which is the cycle that most often
/// catches a naive resolver.
#[test]
fn mutually_referencing_documents_resolve() {
    let a = Schema::object()
        .id("https://example.com/a")
        .property("b", Schema::reference("https://example.com/b"))
        .build()
        .unwrap();
    let b = Schema::object()
        .id("https://example.com/b")
        .property("a", Schema::reference("https://example.com/a"))
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&a).unwrap();
    registry.insert(&b).unwrap();

    registry.check_references().unwrap();
    assert!(
        registry
            .resolve("https://example.com/a#/properties/b")
            .is_some()
    );
}

/// A `$dynamicAnchor` is reachable, and an ordinary `$ref` may name it too.
#[test]
fn a_dynamic_anchor_is_reachable_by_both_reference_forms() {
    let schema = Schema::object()
        .id("https://example.com/tree")
        .dynamic_anchor("node")
        .property(
            "children",
            Schema::array().items(Schema::builder().dynamic_ref("#node")),
        )
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&schema).unwrap();

    assert!(
        registry
            .resolve_dynamic("https://example.com/tree#node")
            .is_some()
    );
    // Per 2020-12 core, a `$dynamicAnchor` is also a plain-name `$ref` target.
    assert!(registry.resolve("https://example.com/tree#node").is_some());

    registry.check_references().unwrap();
}

/// When several resources carry one `$dynamicAnchor`, the outermost — earliest
/// inserted — wins.
#[test]
fn the_outermost_resource_wins_a_dynamic_anchor() {
    let outer = Schema::object()
        .id("https://example.com/outer")
        .dynamic_anchor("node")
        .title("outer")
        .build()
        .unwrap();
    let inner = Schema::object()
        .id("https://example.com/inner")
        .dynamic_anchor("node")
        .title("inner")
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&outer).unwrap();
    registry.insert(&inner).unwrap();

    // Asking the inner resource still hands back the outermost holder.
    let resolved = registry
        .resolve_dynamic("https://example.com/inner#node")
        .unwrap();
    assert_eq!(resolved.get("title"), Some(&serde_json::json!("outer")));

    // A plain `$ref` is not dynamic and stays where it was pointed.
    let statically = registry.resolve("https://example.com/inner#node").unwrap();
    assert_eq!(statically.get("title"), Some(&serde_json::json!("inner")));
}

/// A property whose *name* looks like a keyword is not mistaken for one — the
/// registry must not index `/properties/$id` as a nested resource.
#[test]
fn a_property_named_like_a_keyword_does_not_declare_a_resource() {
    let schema = Schema::object()
        .id("https://example.com/s")
        .property("$id", Schema::string())
        .property("$ref", Schema::string())
        .property("$anchor", Schema::string())
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&schema).unwrap();

    // Exactly one resource: the document itself.
    assert_eq!(registry.len(), 1);
    assert!(registry.contains("https://example.com/s"));
    // And no references were collected from those property names.
    assert_eq!(registry.references().count(), 0);
    registry.check_references().unwrap();
}

/// A document from outside this crate indexes the same way.
#[test]
fn a_parsed_document_indexes_like_a_built_one() {
    let source = r##"{
        "$id": "https://example.com/parsed",
        "type": "object",
        "$defs": {"Id": {"type": "string", "format": "uuid"}},
        "properties": {"id": {"$ref": "#/$defs/Id"}}
    }"##;

    let parsed = Schema::parse(source).unwrap();
    let mut registry = Registry::new();
    registry.insert(&parsed).unwrap();

    registry.check_references().unwrap();
    let target = registry
        .resolve("https://example.com/parsed#/$defs/Id")
        .unwrap();
    assert_eq!(target.get("format"), Some(&serde_json::json!("uuid")));
}

/// Resources report in insertion order, which `resolve_dynamic` depends on.
#[test]
fn resources_are_listed_in_insertion_order() {
    let mut registry = Registry::new();
    for name in ["c", "a", "b"] {
        let schema = Schema::object()
            .id(format!("https://example.com/{name}"))
            .build()
            .unwrap();
        registry.insert(&schema).unwrap();
    }

    assert_eq!(
        registry.resources().collect::<Vec<_>>(),
        [
            "https://example.com/c",
            "https://example.com/a",
            "https://example.com/b",
        ],
    );
}

/// A reference recorded by the registry knows where it was written.
#[test]
fn a_reference_records_its_own_location() {
    let schema = Schema::object()
        .id("https://example.com/s")
        .property("a", Schema::reference("#/$defs/A"))
        .def("A", Schema::string())
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&schema).unwrap();

    let reference = registry.references().next().unwrap();
    assert_eq!(reference.resource, "https://example.com/s");
    assert_eq!(reference.location, "/properties/a");
    assert_eq!(reference.target, "#/$defs/A");
    assert!(!reference.dynamic);
    assert_eq!(reference.absolute(), "https://example.com/s#/$defs/A");
}

/// An empty registry is well defined.
#[test]
fn an_empty_registry_resolves_nothing_and_reports_no_violations() {
    let registry = Registry::new();

    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
    assert!(registry.resolve("https://example.com/s").is_none());
    assert_eq!(registry.references().count(), 0);
    registry.check_references().unwrap();
}

/// A document nested far past anything hand-written must not abort the process.
///
/// `serde_json::Value` clones, drops, and serializes recursively, so a registry
/// that copied what it indexed would overflow the stack — which cannot be
/// caught and takes the whole process with it. Sharing the document instead
/// keeps indexing flat, and the walk stops at its own depth cap.
#[test]
fn a_deeply_nested_document_is_indexed_without_overflowing() {
    // Well past the walk's own depth cap, and past what indexing survived
    // before resources shared their document instead of copying it.
    //
    // Constructing and dropping a `Value` this deep is itself recursive, and a
    // test thread's default 2 MiB stack is not enough for it — so the whole
    // case runs on a thread with room. What is under test is the indexing,
    // which must stay flat however deep the document goes.
    let indexed = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let mut document = serde_json::json!({"type": "string"});
            for _ in 0..1_000 {
                document = serde_json::json!({"properties": {"n": document}});
            }

            let mut registry = Registry::new();
            registry.insert_shared(Arc::new(document)).unwrap();

            (registry.len(), registry.contains(ANONYMOUS))
        })
        .expect("spawning the deep-document thread")
        .join()
        .expect("indexing a deep document must not abort");

    assert_eq!(indexed, (1, true));
}

/// Indexing shares the document rather than copying it per resource.
#[test]
fn nested_resources_share_one_document() {
    let schema = Schema::object()
        .id("https://example.com/root")
        .def("A", Schema::object().id("https://example.com/a"))
        .def("B", Schema::object().id("https://example.com/b"))
        .build()
        .unwrap();
    let document = Arc::new(schema.to_value());

    let mut registry = Registry::new();
    registry.insert_shared(Arc::clone(&document)).unwrap();

    assert_eq!(registry.len(), 3);
    // The three resources hold the one document between them, plus this
    // binding — no per-resource copies were made.
    assert_eq!(Arc::strong_count(&document), 4);
}

/// A nested resource resolves pointers against its own root, not the document's.
#[test]
fn a_nested_resource_resolves_pointers_from_its_own_root() {
    let schema = Schema::object()
        .id("https://example.com/root")
        .title("the root")
        .def(
            "Inner",
            Schema::object()
                .id("https://example.com/inner")
                .title("the inner one")
                .def("Deep", Schema::string().min_length(7)),
        )
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&schema).unwrap();

    // `#` on the inner resource is the inner schema, not the document root.
    let inner = registry.resolve("https://example.com/inner").unwrap();
    assert_eq!(
        inner.get("title"),
        Some(&serde_json::json!("the inner one"))
    );

    let deep = registry
        .resolve("https://example.com/inner#/$defs/Deep")
        .unwrap();
    assert_eq!(deep.get("minLength"), Some(&serde_json::json!(7)));

    let root = registry.resolve("https://example.com/root").unwrap();
    assert_eq!(root.get("title"), Some(&serde_json::json!("the root")));
}

/// A dangling `$ref` is a *valid* 2020-12 document with a broken reference, and
/// must not be reported as a malformed one.
#[test]
fn a_dangling_reference_is_not_reported_as_a_malformed_schema() {
    let schema = Schema::object()
        .id("https://example.com/s")
        .property("a", Schema::reference("#/$defs/Nope"))
        .build()
        .unwrap();

    // The meta-schema is content with it.
    schema.validate().unwrap();

    let mut registry = Registry::new();
    registry.insert(&schema).unwrap();
    let error = registry.check_references().unwrap_err();

    assert_eq!(error.kind(), ErrorKind::References);
    assert!(error.to_string().contains("resolve to nothing"));
    assert!(!error.to_string().contains("not valid JSON Schema"));
}

/// The meta-schema check still reports under its own headline.
#[test]
fn a_malformed_schema_still_reports_as_one() {
    let error = Schema::builder()
        .anchor("not a valid anchor")
        .build()
        .unwrap_err();

    assert_eq!(error.kind(), ErrorKind::MetaSchema);
    assert!(error.to_string().contains("not valid JSON Schema 2020-12"));
}

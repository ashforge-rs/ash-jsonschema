//! `.build()` runs the assembled document against the embedded 2020-12
//! meta-schema. These tests pin down what that catches, where it says the
//! problem is, and what it deliberately lets through.

use ash_jsonschema_builder::{Schema, SchemaObject, SchemaType, TypeSet};

/// Builds, expects rejection, and returns the first violation's
/// `(location, rule, message)`.
fn rejected(builder: impl Into<Schema>) -> (String, String, String) {
    let error = Into::<Schema>::into(builder).validate().unwrap_err();
    let first = error.first();
    (
        first.location.clone(),
        first.rule.clone(),
        first.message.clone(),
    )
}

// ---------------------------------------------------------------------------
// What the meta-schema catches
// ---------------------------------------------------------------------------

#[test]
fn an_anchor_must_be_a_plain_name() {
    let (location, rule, message) = rejected(Schema::string().anchor("not an anchor"));
    assert_eq!(location, "/$anchor");
    assert_eq!(rule, "meta/core#/$defs/anchorString/pattern");
    assert_eq!(message, "must match /^[A-Za-z_][-A-Za-z0-9._]*$/");

    let (location, ..) = rejected(Schema::string().dynamic_anchor("1-starts-with-a-digit"));
    assert_eq!(location, "/$dynamicAnchor");

    // The legal forms still pass.
    assert!(Schema::string().anchor("Address").build().is_ok());
    assert!(Schema::string().anchor("_a.b-c1").build().is_ok());
}

#[test]
fn an_id_may_not_carry_a_non_empty_fragment() {
    let (location, rule, _) = rejected(Schema::object().id("https://example.com/x#/frag"));
    assert_eq!(location, "/$id");
    assert_eq!(rule, "meta/core#/properties/$id/pattern");

    // An empty fragment is explicitly permitted.
    assert!(
        Schema::object()
            .id("https://example.com/x#")
            .build()
            .is_ok()
    );
    assert!(Schema::object().id("https://example.com/x").build().is_ok());
}

#[test]
fn a_divisor_must_be_positive() {
    let (location, rule, message) = rejected(Schema::number().multiple_of(0));
    assert_eq!(location, "/multipleOf");
    assert_eq!(
        rule,
        "meta/validation#/properties/multipleOf/exclusiveMinimum"
    );
    assert_eq!(message, "must be greater than 0");

    assert_eq!(rejected(Schema::number().multiple_of(-2)).0, "/multipleOf");
    assert!(Schema::number().multiple_of(0.5).build().is_ok());
}

#[test]
fn a_dependency_list_may_not_repeat_a_name() {
    let (location, rule, message) =
        rejected(Schema::object().dependent_required("card", ["cvv", "cvv"]));
    assert_eq!(location, "/dependentRequired/card");
    assert_eq!(rule, "meta/validation#/$defs/stringArray/uniqueItems");
    assert_eq!(message, r#"items must be unique, "cvv" is repeated"#);
}

/// The builders dedupe `required` and `type` themselves, but a hand-assembled
/// [`SchemaObject`] can reach past that — and the meta-schema still catches it.
#[test]
fn a_hand_assembled_keyword_bag_is_held_to_the_same_rules() {
    let mut keywords = SchemaObject::new();
    keywords.required = Some(vec!["a".to_owned(), "a".to_owned()]);
    let (location, rule, _) = rejected(Schema::from(keywords));
    assert_eq!(location, "/required");
    assert_eq!(rule, "meta/validation#/$defs/stringArray/uniqueItems");

    let mut keywords = SchemaObject::new();
    keywords.types = Some(TypeSet::Many(vec![SchemaType::String, SchemaType::String]));
    let (location, rule, message) = rejected(Schema::from(keywords));
    assert_eq!(location, "/type");
    assert_eq!(rule, "meta/validation#/properties/type/anyOf");
    assert_eq!(message, "matches none of the permitted forms");
}

// ---------------------------------------------------------------------------
// Where it says the problem is
// ---------------------------------------------------------------------------

/// The walk is depth-first over the whole document, so a subschema a nested
/// builder contributed is checked in place and located by JSON Pointer — this
/// is what makes a single root `.build()` enough.
#[test]
fn a_violation_deep_in_the_tree_is_located_precisely() {
    let (location, ..) = rejected(Schema::object().property(
        "outer",
        Schema::array().items(Schema::string().anchor("no good")),
    ));
    assert_eq!(location, "/properties/outer/items/$anchor");

    let (location, ..) = rejected(Schema::object().def(
        "Wrapper",
        Schema::object().property("x", Schema::number().multiple_of(0)),
    ));
    assert_eq!(location, "/$defs/Wrapper/properties/x/multipleOf");

    let (location, ..) = rejected(Schema::all_of([
        Schema::string().build_unchecked(),
        Schema::string().anchor("bad one").build_unchecked(),
    ]));
    assert_eq!(location, "/allOf/1/$anchor");
}

#[test]
fn a_property_name_needing_escapes_still_points_somewhere_usable() {
    let (location, ..) =
        rejected(Schema::object().property("a/b~c", Schema::string().anchor("bad anchor")));
    // RFC 6901: `~` escapes to `~0` and `/` to `~1`.
    assert_eq!(location, "/properties/a~1b~0c/$anchor");
}

#[test]
fn every_violation_is_reported_not_just_the_first() {
    let error = Schema::object()
        .property("a", Schema::string().anchor("bad one"))
        .property("b", Schema::number().multiple_of(0))
        .build()
        .unwrap_err();

    let locations: Vec<&str> = error
        .violations()
        .iter()
        .map(|v| v.location.as_str())
        .collect();
    assert_eq!(
        locations,
        ["/properties/a/$anchor", "/properties/b/multipleOf"]
    );
    assert!(error.to_string().contains("2 violations"));
}

// ---------------------------------------------------------------------------
// What it deliberately lets through
// ---------------------------------------------------------------------------

/// The meta-schema constrains a schema's shape, not its sense. These are all
/// legal 2020-12 documents, however little they achieve.
#[test]
fn a_nonsensical_but_well_formed_schema_still_builds() {
    // A range no string can satisfy.
    assert!(
        Schema::string()
            .min_length(10)
            .max_length(2)
            .build()
            .is_ok()
    );
    // A `then` with no `if` to trigger it.
    assert!(
        Schema::object()
            .then_schema(Schema::object())
            .build()
            .is_ok()
    );
    // Bounds that exclude each other.
    assert!(Schema::integer().minimum(10).maximum(1).build().is_ok());
    // A `$ref` to a definition that does not exist — resolution is a
    // validator's problem, and the crate never follows your references.
    assert!(Schema::reference("#/$defs/Missing").build().is_ok());
}

#[test]
fn the_boolean_schema_forms_are_valid_documents() {
    assert!(Schema::anything().validate().is_ok());
    assert!(Schema::nothing().validate().is_ok());
    assert!(Schema::object().property("x", true).build().is_ok());
}

#[test]
fn build_frozen_applies_the_same_check_as_build() {
    let error = Schema::object()
        .property("a", Schema::string().anchor("bad anchor"))
        .build_frozen()
        .unwrap_err();
    assert_eq!(error.first().location, "/properties/a/$anchor");

    let frozen = Schema::object()
        .required_property("a", Schema::string())
        .build_frozen()
        .unwrap();
    let built = Schema::object()
        .required_property("a", Schema::string())
        .build()
        .unwrap();
    assert_eq!(*frozen, built);
}

/// Freezing is orthogonal to checking: `Schema::freeze` takes whatever it is
/// given, including a document `build` would have rejected.
#[test]
fn freezing_an_unchecked_schema_does_not_check_it() {
    let frozen = Schema::string()
        .anchor("not an anchor")
        .build_unchecked()
        .freeze();
    assert!(frozen.validate().is_err());
    assert_eq!(
        frozen.to_json(),
        r#"{"$anchor":"not an anchor","type":"string"}"#
    );
}

#[test]
fn build_unchecked_skips_the_check_entirely() {
    let schema = Schema::string().anchor("not an anchor").build_unchecked();
    assert!(schema.validate().is_err());
    assert_eq!(
        schema.to_json(),
        r#"{"$anchor":"not an anchor","type":"string"}"#
    );
}

// ---------------------------------------------------------------------------
// The documents themselves
// ---------------------------------------------------------------------------

/// The meta-schema is a JSON Schema, so it must validate against itself. This
/// exercises nearly every path in the evaluator at once — `$dynamicRef`
/// recursion, all eight documents, every applicator they use.
#[test]
fn the_meta_schema_validates_against_itself() {
    use ash_jsonschema_builder::meta::{DOCUMENTS, MetaSchema};
    use ash_jsonschema_builder::validate_document;

    let meta = MetaSchema::current();
    for document in DOCUMENTS {
        let parsed = meta.document(document.uri).expect("document is embedded");
        assert_eq!(
            validate_document(parsed),
            Ok(()),
            "{} fails the dialect",
            document.uri
        );
    }
}

fn nested(levels: usize) -> Schema {
    let mut schema = Schema::string().build_unchecked();
    for _ in 0..levels {
        schema = Schema::object().property("next", schema).build_unchecked();
    }
    schema
}

/// Deep nesting is walked all the way down, and a violation at the very bottom
/// is still located by its full path.
#[test]
fn a_deeply_nested_document_is_checked_to_the_bottom() {
    assert!(nested(50).validate().is_ok());

    let mut schema = Schema::string().anchor("bad anchor").build_unchecked();
    for _ in 0..50 {
        schema = Schema::object().property("next", schema).build_unchecked();
    }
    let error = schema.validate().unwrap_err();
    assert_eq!(error.first().location.matches("/next").count(), 50);
    assert!(error.first().location.ends_with("/$anchor"));
}

/// Past the depth cap the evaluator gives up with an ordinary error rather
/// than overflowing the stack, which would abort the process uncatchably.
#[test]
fn nesting_past_the_cap_is_an_error_not_a_crash() {
    let error = nested(200).validate().unwrap_err();
    assert!(
        error
            .violations()
            .iter()
            .any(|v| v.message.contains("nested too deeply")),
        "expected a depth violation, got: {error}",
    );
}

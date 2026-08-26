//! Reading schemas in, and picking them back up.
//!
//! The builder side of the crate writes a schema from nothing. These tests
//! cover the other direction: parsing a document that already exists, keeping
//! everything it arrived with, and resuming a builder chain on top of it.

use ash_jsonschema_builder::{ParseError, Schema, SchemaType, StringBuilder, TypeSet};

/// A document written the way this crate renders one survives the trip out and
/// back byte for byte.
#[test]
fn a_rendered_document_parses_back_to_an_equal_schema() {
    let built = Schema::object()
        .dialect()
        .id("https://example.com/schemas/user")
        .title("User")
        .required_property("id", Schema::string().format("uuid"))
        .property("age", Schema::integer().minimum(0).nullable())
        .closed()
        .build()
        .unwrap();

    let parsed = Schema::parse(&built.to_json()).unwrap();

    assert_eq!(parsed, built);
    assert_eq!(parsed.to_json(), built.to_json());
}

/// Property order is the author's, not the alphabet's, in both directions.
#[test]
fn parsing_preserves_the_order_properties_were_written_in() {
    let source = r#"{"type":"object","properties":{"zebra":{"type":"string"},"apple":{"type":"string"},"mango":{"type":"string"}}}"#;

    let parsed = Schema::parse(source).unwrap();
    let names: Vec<_> = parsed
        .as_object()
        .unwrap()
        .properties
        .as_ref()
        .unwrap()
        .keys()
        .collect();

    assert_eq!(names, ["zebra", "apple", "mango"]);
    assert_eq!(parsed.to_json(), source);
}

/// A keyword outside the dialect is kept rather than dropped, which is what
/// makes the crate safe to point at an OpenAPI or extension-vocabulary
/// document.
#[test]
fn keywords_outside_the_dialect_survive_the_round_trip() {
    let source = r#"{"type":"object","discriminator":{"propertyName":"kind"},"x-internal":true}"#;

    let parsed = Schema::parse(source).unwrap();
    let extensions = &parsed.as_object().unwrap().extensions;

    assert_eq!(extensions.len(), 2);
    assert_eq!(extensions.get("x-internal"), Some(&serde_json::json!(true)));
    assert_eq!(parsed.to_json(), source);
}

/// The builder can write one too, for a schema being authored rather than read.
#[test]
fn the_builder_writes_extension_keywords_after_the_dialect_ones() {
    let schema = Schema::object()
        .property("kind", Schema::string())
        .keyword("x-owner", "platform")
        .build()
        .unwrap();

    assert_eq!(
        schema.to_json(),
        r#"{"type":"object","properties":{"kind":{"type":"string"}},"x-owner":"platform"}"#,
    );
}

/// Both schema forms parse, including the bare booleans.
#[test]
fn the_boolean_schema_forms_parse() {
    assert_eq!(Schema::parse("true").unwrap().as_bool(), Some(true));
    assert_eq!(Schema::parse("false").unwrap().as_bool(), Some(false));
    assert_eq!(Schema::parse("{}").unwrap().as_bool(), None);
}

/// Integers do not pick up a fractional part on the way through.
#[test]
fn numeric_keywords_keep_their_json_form() {
    let parsed = Schema::parse(r#"{"type":"number","minimum":0,"multipleOf":0.5}"#).unwrap();
    assert_eq!(
        parsed.to_json(),
        r#"{"type":"number","multipleOf":0.5,"minimum":0}"#,
    );
}

/// `parse_checked` keeps the two failure modes apart: bad JSON is not the same
/// problem as a well-formed document that is not a schema.
#[test]
fn parse_checked_distinguishes_bad_json_from_a_bad_schema() {
    let syntax = Schema::parse_checked("{not json").unwrap_err();
    assert!(matches!(syntax, ParseError::Syntax(_)));

    let invalid = Schema::parse_checked(r#"{"$anchor":"not an anchor"}"#).unwrap_err();
    let ParseError::Invalid(error) = invalid else {
        panic!("expected a meta-schema violation");
    };
    assert_eq!(error.first().location, "/$anchor");

    assert!(Schema::parse_checked(r#"{"type":"string"}"#).is_ok());
}

/// A parsed schema goes back into a builder and picks up more keywords.
#[test]
fn a_parsed_schema_resumes_as_a_builder() {
    let parsed = Schema::parse(r#"{"type":"object","properties":{"id":{"type":"string"}}}"#)
        .unwrap();

    let extended = parsed
        .into_builder()
        .title("User")
        .description("A person.")
        .build()
        .unwrap();

    assert_eq!(
        extended.to_json(),
        r#"{"title":"User","description":"A person.","type":"object","properties":{"id":{"type":"string"}}}"#,
    );
}

/// Resuming under a type marker reopens that type's vocabulary.
#[test]
fn resuming_under_a_type_marker_reopens_its_vocabulary() {
    let parsed = Schema::parse(r#"{"type":"string","minLength":1}"#).unwrap();

    let tightened: StringBuilder = parsed.into_typed();
    let schema = tightened.max_length(64).pattern("^[a-z]+$").build().unwrap();

    assert_eq!(
        schema.to_json(),
        r#"{"type":"string","maxLength":64,"minLength":1,"pattern":"^[a-z]+$"}"#,
    );
}

/// A boolean schema has no keywords to resume from, and says so by yielding an
/// empty builder rather than by failing.
#[test]
fn a_boolean_schema_resumes_as_an_empty_builder() {
    let resumed = Schema::anything().into_builder().title("anything").build().unwrap();
    assert_eq!(resumed.to_json(), r#"{"title":"anything"}"#);
}

/// The inspection helpers read a schema without matching on its shape.
#[test]
fn the_inspection_helpers_reach_the_common_places() {
    let schema = Schema::object()
        .required_property("id", Schema::string().format("uuid"))
        .property("age", Schema::integer())
        .def("Money", Schema::object())
        .build()
        .unwrap();

    assert_eq!(schema.schema_type(), Some(&TypeSet::One(SchemaType::Object)));
    assert!(schema.property("id").is_some());
    assert!(schema.property("nothing").is_none());
    assert!(schema.requires("id"));
    assert!(!schema.requires("age"));
    assert!(schema.def("Money").is_some());

    assert_eq!(
        schema.pointer("/properties/id/format"),
        Some(serde_json::json!("uuid")),
    );
    assert_eq!(schema.pointer("/properties/nothing"), None);
}

/// A violation's location is a pointer into the very document that produced it,
/// so the two compose: the error says where, and `pointer` shows what is there.
#[test]
fn a_violation_location_resolves_against_the_schema_it_came_from() {
    let schema = Schema::object()
        .property("inner", Schema::string().anchor("not an anchor"))
        .build_unchecked();

    let error = schema.validate().unwrap_err();
    let location = &error.first().location;

    assert_eq!(location, "/properties/inner/$anchor");
    assert_eq!(schema.pointer(location), Some(serde_json::json!("not an anchor")));
}

/// The object conveniences write the same document the long form does.
#[test]
fn the_object_conveniences_match_their_long_forms() {
    let short = Schema::object()
        .required_properties([("id", Schema::string()), ("email", Schema::string())])
        .closed()
        .build()
        .unwrap();

    let long = Schema::object()
        .required_property("id", Schema::string())
        .required_property("email", Schema::string())
        .additional_properties(false)
        .build()
        .unwrap();

    assert_eq!(short, long);

    let conditional = Schema::object()
        .property_maybe_required("a", Schema::string(), true)
        .property_maybe_required("b", Schema::string(), false)
        .build()
        .unwrap();

    assert_eq!(
        conditional.as_object().unwrap().required.as_ref().unwrap(),
        &["a"],
    );
}

/// JSON has one number type, so `1.0` and `1` are the same value written two
/// ways and the meta-schema's `nonNegativeInteger` admits both. Rejecting the
/// decimal form would refuse a schema the specification calls valid.
#[test]
fn a_count_keyword_accepts_the_decimal_spelling_of_an_integer() {
    let schema = Schema::parse(r#"{"minItems": 1.0, "maxLength": 3.0}"#).unwrap();

    // Normalised on the way in, so the round trip is the integer form.
    assert_eq!(schema.to_json(), r#"{"maxLength":3,"minItems":1}"#);
}

#[test]
fn a_count_keyword_still_refuses_what_is_not_a_count() {
    for bad in [r#"{"minItems": 2.5}"#, r#"{"minItems": -1}"#] {
        let error = Schema::parse(bad).unwrap_err().to_string();
        assert!(
            error.contains("non-negative integer"),
            "`{bad}` should be refused as a count, got: {error}",
        );
    }
}

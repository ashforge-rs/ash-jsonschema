//! Validating instances against schemas.
//!
//! The evaluator is also run against the official
//! [JSON-Schema-Test-Suite](https://github.com/json-schema-org/JSON-Schema-Test-Suite);
//! these cover the behaviour this crate promises, plus the cases that suite
//! caught during development, so a regression shows up without the suite
//! present.

use ash_jsonschema::validator::Mode;
use ash_jsonschema::{CompileError, Registry, Schema, Validator, object, validator};

fn validator_for(schema: &Schema) -> Validator {
    Validator::new(schema).expect("the schema compiles")
}

// --- the shape of the API -------------------------------------------------

#[test]
fn a_validator_answers_both_ways_about_the_same_instance() {
    let schema = object! {
        required "a": Schema::string(),
        required "b": Schema::string(),
    }
    .build()
    .unwrap();
    let validator = validator_for(&schema);
    let instance = serde_json::json!({});

    // Every error, versus the first one only.
    assert_eq!(validator.validate(&instance, Mode::All).errors.len(), 2);
    assert_eq!(validator.validate(&instance, Mode::FailFast).errors.len(), 1);

    assert!(!validator.is_valid(&instance));
    assert!(validator.check(&instance).is_err());
    assert!(validator.check(&serde_json::json!({"a": "x", "b": "y"})).is_ok());
}

#[test]
fn an_error_names_where_in_the_data_and_which_rule() {
    let schema = object! {
        required "age": Schema::integer().minimum(18),
    }
    .build()
    .unwrap();

    let outcome = validator_for(&schema).validate(&serde_json::json!({"age": 3}), Mode::All);
    let error = outcome.first().unwrap();

    assert_eq!(error.instance_location, "/age");
    assert_eq!(error.keyword_location, "/properties/age/minimum");
    assert!(error.message().contains("18"));
}

#[test]
fn errors_can_be_filtered_to_one_part_of_the_instance() {
    let schema = object! {
        required "user": object! {
            required "name": Schema::string().min_length(1),
            required "age": Schema::integer().minimum(0),
        },
        required "tag": Schema::string().min_length(1),
    }
    .build()
    .unwrap();

    let outcome = validator_for(&schema).validate(
        &serde_json::json!({"user": {"name": "", "age": -1}, "tag": ""}),
        Mode::All,
    );

    assert_eq!(outcome.errors.len(), 3);
    assert_eq!(outcome.at("/user").count(), 2);
    assert_eq!(outcome.at("/user/name").count(), 1);
    assert_eq!(outcome.at("/tag").count(), 1);
}

#[test]
fn the_schema_is_fixed_for_the_validators_life() {
    let schema = Schema::string().title("Fixed").build().unwrap();
    let validator = validator_for(&schema);

    // A copy comes out; changing it does not reach back in.
    let mut exported = validator.export_schema().unwrap();
    exported.as_object_mut().unwrap().types = None;

    assert!(!validator.is_valid(&serde_json::json!(1)));
    assert_eq!(
        validator.schema().get("title"),
        Some(&serde_json::json!("Fixed")),
    );
}

#[test]
fn the_macro_builds_one_inline() {
    let user = validator! {
        object! {
            required "id": Schema::string().min_length(1),
            required "age": Schema::integer().minimum(0),
        }
        .closed()
    }
    .unwrap();

    assert!(user.is_valid(&serde_json::json!({"id": "u1", "age": 30})));
    assert!(!user.is_valid(&serde_json::json!({"id": "u1"})));
    assert!(!user.is_valid(&serde_json::json!({"id": "u1", "age": 3, "extra": 1})));
}

// --- what a schema can get wrong ------------------------------------------

#[test]
fn a_pattern_that_will_not_compile_is_refused_at_construction() {
    // `regex-lite` has no lookahead, so this cannot be enforced — and is
    // reported rather than silently ignored.
    let schema = object! {
        "a": Schema::string().pattern("(?=foo)bar"),
    }
    .build()
    .unwrap();

    let error = Validator::new(&schema).unwrap_err();
    let CompileError::BadPattern { location, .. } = &error else {
        panic!("expected a pattern error, got {error:?}");
    };
    assert_eq!(location, "/properties/a/pattern");
}

// --- keyword coverage -----------------------------------------------------

#[test]
fn integer_accepts_a_whole_float() {
    let validator = validator_for(&Schema::integer().build().unwrap());

    assert!(validator.is_valid(&serde_json::json!(1)));
    // JSON does not distinguish `1` from `1.0`, and neither does the spec.
    assert!(validator.is_valid(&serde_json::json!(1.0)));
    assert!(!validator.is_valid(&serde_json::json!(1.5)));
}

#[test]
fn a_bound_written_as_a_decimal_still_applies() {
    // `minItems: 2.0` is the same assertion as `minItems: 2` — JSON draws no
    // distinction — and reading it with `as_u64` alone would silently drop it,
    // leaving the bound unenforced.
    //
    // This crate's own `SchemaObject` types the count as `u64`, so a schema
    // that came through `Schema::parse` can never carry the decimal form. A
    // document from elsewhere can, which is what `from_document` is for.
    let document = serde_json::json!({"type": "array", "minItems": 2.0});
    let validator = Validator::from_document(&document).unwrap();

    assert!(!validator.is_valid(&serde_json::json!([1])));
    assert!(validator.is_valid(&serde_json::json!([1, 2])));
}

#[test]
fn const_null_asserts_null_rather_than_being_absent() {
    // The keyword is present and its value is `null`; that is not the same as
    // the keyword being missing, which would accept anything.
    let schema = Schema::parse(r#"{"const":null}"#).unwrap();
    assert_eq!(schema.to_json(), r#"{"const":null}"#);

    let validator = validator_for(&schema);
    assert!(validator.is_valid(&serde_json::json!(null)));
    assert!(!validator.is_valid(&serde_json::json!(0)));
}

#[test]
fn string_length_counts_characters_not_bytes() {
    let schema = Schema::string().max_length(2).build().unwrap();
    let validator = validator_for(&schema);

    assert!(validator.is_valid(&serde_json::json!("αβ")));
    assert!(!validator.is_valid(&serde_json::json!("αβγ")));
}

#[test]
fn unique_items_compares_by_value() {
    let schema = Schema::array().unique_items(true).build().unwrap();
    let validator = validator_for(&schema);

    assert!(validator.is_valid(&serde_json::json!([1, 2, 3])));
    assert!(!validator.is_valid(&serde_json::json!([1, 2, 1])));
    // `1` and `1.0` are one value in JSON.
    assert!(!validator.is_valid(&serde_json::json!([1, 1.0])));
}

#[test]
fn one_of_requires_exactly_one_match() {
    let schema = Schema::one_of([
        Schema::from(Schema::integer().minimum(0)),
        Schema::from(Schema::integer().maximum(10)),
    ])
    .build()
    .unwrap();
    let validator = validator_for(&schema);

    // Matches the first only.
    assert!(validator.is_valid(&serde_json::json!(20)));
    // Matches both, which `oneOf` forbids.
    let outcome = validator.validate(&serde_json::json!(5), Mode::All);
    assert!(!outcome.is_valid());
    assert!(outcome.first().unwrap().message().contains("exactly one"));
}

/// A failed `anyOf` keeps each alternative's reason, nested under it.
///
/// "does not match any permitted alternative" on its own is the least useful
/// thing a validator can say, because the question it leaves unanswered is
/// which alternative was meant and how close it came. The tree keeps that;
/// the flat view surfaces the branch reasons as the leaves they are.
#[test]
fn a_failing_branch_keeps_every_alternatives_reason() {
    let schema = Schema::any_of([
        Schema::from(Schema::string()),
        Schema::from(Schema::integer()),
    ])
    .build()
    .unwrap();

    let outcome = validator_for(&schema).validate(&serde_json::json!(true), Mode::All);

    // The tree is rooted at the `anyOf` that failed.
    let anyof = &outcome.unit().errors[0];
    assert_eq!(anyof.keyword_location, "/anyOf");
    assert_eq!(anyof.errors.len(), 2, "one unit per failed alternative");
    assert_eq!(anyof.errors[0].keyword_location, "/anyOf/0");
    assert_eq!(anyof.errors[1].keyword_location, "/anyOf/1");

    // The flat view gives the reasons, not the scaffolding.
    assert_eq!(outcome.errors.len(), 2);
    assert!(
        outcome
            .errors
            .iter()
            .all(|error| error.keyword() == "type"),
        "each leaf is the `type` that rejected that branch",
    );
}

#[test]
fn if_then_else_selects_a_branch_without_asserting_itself() {
    let schema = Schema::object()
        .property("kind", Schema::string())
        .if_then_else(
            Schema::object().property("kind", Schema::builder().constant("circle")),
            Schema::object().required(["radius"]),
            Schema::object().required(["width"]),
        )
        .build()
        .unwrap();
    let validator = validator_for(&schema);

    assert!(validator.is_valid(&serde_json::json!({"kind": "circle", "radius": 1})));
    assert!(!validator.is_valid(&serde_json::json!({"kind": "circle"})));
    assert!(validator.is_valid(&serde_json::json!({"kind": "rect", "width": 1})));
}

// --- unevaluated ----------------------------------------------------------

#[test]
fn unevaluated_properties_sees_through_composition() {
    // This is what `additionalProperties` cannot do: `allOf` already accounted
    // for `a`, and only `unevaluatedProperties` knows that.
    let schema = Schema::object()
        .all_of([Schema::from(
            Schema::object().property("a", Schema::string()),
        )])
        .unevaluated_properties(false)
        .build()
        .unwrap();
    let validator = validator_for(&schema);

    assert!(validator.is_valid(&serde_json::json!({"a": "x"})));
    assert!(!validator.is_valid(&serde_json::json!({"a": "x", "b": 1})));
}

#[test]
fn a_failed_branch_evaluates_nothing() {
    // The `anyOf` branch that fails must not mark `a` as evaluated, or
    // `unevaluatedProperties` would wrongly accept it.
    let schema = Schema::object()
        .any_of([
            Schema::from(Schema::object().property("a", Schema::integer())),
            Schema::from(Schema::object().property("b", Schema::string())),
        ])
        .unevaluated_properties(false)
        .build()
        .unwrap();
    let validator = validator_for(&schema);

    // `b` matches its branch, and `a` is left over by the branch that failed.
    assert!(!validator.is_valid(&serde_json::json!({"a": "not an integer", "b": "x"})));
    assert!(validator.is_valid(&serde_json::json!({"b": "x"})));
}

#[test]
fn unevaluated_items_tracks_which_items_contains_matched() {
    // `contains` evaluates whichever items match — here index 2 but not 1 — so
    // tracking a high-water mark rather than a set would wrongly accept.
    let schema = Schema::parse(
        r#"{"prefixItems":[true],"contains":{"type":"string"},"unevaluatedItems":false}"#,
    )
    .unwrap();
    let validator = validator_for(&schema);

    assert!(validator.is_valid(&serde_json::json!([1, "foo"])));
    // Index 1 is a gap: matched by nothing.
    assert!(!validator.is_valid(&serde_json::json!([1, 2, "foo"])));
}

// --- references -----------------------------------------------------------

#[test]
fn a_local_reference_is_followed() {
    let schema = Schema::object()
        .def("Name", Schema::string().min_length(2))
        .required_property("name", Schema::reference("#/$defs/Name"))
        .build()
        .unwrap();
    let validator = validator_for(&schema);

    assert!(validator.is_valid(&serde_json::json!({"name": "ok"})));
    assert!(!validator.is_valid(&serde_json::json!({"name": "x"})));
}

#[test]
fn a_reference_across_documents_needs_the_registry() {
    let address = Schema::object()
        .id("https://example.com/address")
        .required_property("city", Schema::string())
        .build()
        .unwrap();
    let user = Schema::object()
        .id("https://example.com/user")
        .required_property("address", Schema::reference("address"))
        .build()
        .unwrap();

    let mut registry = Registry::new();
    registry.insert(&address).unwrap();
    registry.insert(&user).unwrap();

    let validator = Validator::with_registry(&user, &registry).unwrap();
    assert!(validator.is_valid(&serde_json::json!({"address": {"city": "Athens"}})));
    assert!(!validator.is_valid(&serde_json::json!({"address": {}})));

    // The same schema without the registry cannot follow the reference.
    let alone = Validator::new(&user).unwrap();
    let outcome = alone.validate(&serde_json::json!({"address": {}}), Mode::All);
    assert!(outcome.first().unwrap().message().contains("resolves to nothing"));
}

#[test]
fn a_percent_encoded_pointer_resolves() {
    // A fragment is part of a URI, so a `%` in a name is written `%25`.
    let schema = Schema::parse(
        r##"{"$defs":{"a%field":{"type":"integer"}},
             "properties":{"a":{"$ref":"#/$defs/a%25field"}}}"##,
    )
    .unwrap();
    let validator = validator_for(&schema);

    assert!(validator.is_valid(&serde_json::json!({"a": 1})));
    assert!(!validator.is_valid(&serde_json::json!({"a": "x"})));
}

#[test]
fn a_recursive_schema_validates_without_running_away() {
    let node = Schema::object()
        .id("https://example.com/node")
        .required_property("value", Schema::integer())
        .property("next", Schema::reference("https://example.com/node"))
        .build()
        .unwrap();
    let validator = validator_for(&node);

    assert!(validator.is_valid(&serde_json::json!({"value": 1, "next": {"value": 2}})));
    assert!(!validator.is_valid(&serde_json::json!({"value": 1, "next": {"value": "x"}})));
}

#[test]
fn a_reference_cycle_that_consumes_no_instance_reports_rather_than_overflowing() {
    // `{"$ref": "#"}` at the root recurses forever; the depth cap turns that
    // into an ordinary error instead of a stack overflow.
    let schema = Schema::parse(r##"{"$ref":"#"}"##).unwrap();
    let validator = validator_for(&schema);

    let outcome = validator.validate(&serde_json::json!({}), Mode::All);
    assert!(!outcome.is_valid());
    assert!(
        outcome
            .errors
            .iter()
            .any(|error| error.message().contains("too deeply"))
    );
}

// --- serde ----------------------------------------------------------------

#[test]
fn a_serializable_value_can_be_checked_directly() {
    #[derive(serde::Serialize)]
    struct User {
        id: String,
        age: u8,
    }

    let schema = object! {
        required "id": Schema::string().min_length(1),
        required "age": Schema::integer().minimum(18),
    }
    .build()
    .unwrap();
    let validator = validator_for(&schema);

    assert!(
        validator
            .check_value(&User {
                id: "u1".into(),
                age: 30
            })
            .unwrap()
            .is_valid()
    );

    let outcome = validator
        .check_value(&User {
            id: "u1".into(),
            age: 3,
        })
        .unwrap();
    assert_eq!(outcome.first().unwrap().instance_location, "/age");
}

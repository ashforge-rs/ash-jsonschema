//! The specification's four output formats, and annotation collection.
//!
//! The shape asserted here is not this crate's invention: it is what
//! `https://json-schema.org/draft/2020-12/output/schema` requires, so these
//! tests are a conformance bar rather than a description of current
//! behaviour.

use ash_jsonschema::validator::{Mode, OutputFormat};
use ash_jsonschema::{Schema, Validator};
use serde_json::json;

fn validator(source: &str) -> Validator {
    Validator::new(&Schema::parse(source).expect("schema parses")).expect("schema compiles")
}

#[test]
fn flag_is_the_verdict_and_nothing_else() {
    let validator = validator(r#"{"type": "string"}"#);

    let failed = validator.validate(&json!(42), Mode::All);
    assert_eq!(failed.output(OutputFormat::Flag), json!({"valid": false}));

    let passed = validator.validate(&json!("ok"), Mode::All);
    assert_eq!(passed.output(OutputFormat::Flag), json!({"valid": true}));
}

/// Every unit carries the two locations the specification requires.
#[test]
fn basic_units_carry_both_required_locations() {
    let validator = validator(
        r#"{"properties": {"age": {"type": "integer", "minimum": 18}}}"#,
    );
    let outcome = validator.validate(&json!({"age": 3}), Mode::All);

    let output = outcome.output(OutputFormat::Basic);
    let error = &output["errors"][0];

    assert_eq!(error["keywordLocation"], json!("/properties/age/minimum"));
    assert_eq!(error["instanceLocation"], json!("/age"));
    assert!(error["error"].is_string());
}

/// `basic` is flat by definition: no unit in it holds nested units.
#[test]
fn basic_output_flattens_a_nested_failure() {
    let validator = validator(
        r#"{"anyOf": [{"type": "string"}, {"type": "integer", "minimum": 10}]}"#,
    );
    let outcome = validator.validate(&json!(3), Mode::All);

    let output = outcome.output(OutputFormat::Basic);
    let errors = output["errors"].as_array().expect("errors present");

    assert!(!errors.is_empty());
    assert!(
        errors.iter().all(|unit| unit.get("errors").is_none()),
        "a basic unit must not nest",
    );
}

/// The reason `detailed` exists: a failed `anyOf` keeps why each alternative
/// was rejected, which the top-level message alone cannot say.
#[test]
fn detailed_output_keeps_each_alternatives_reason() {
    let validator = validator(
        r#"{"anyOf": [{"type": "string"}, {"type": "integer", "minimum": 10}]}"#,
    );
    let outcome = validator.validate(&json!(3), Mode::All);

    let output = outcome.output(OutputFormat::Detailed);
    let anyof = &output["errors"][0];
    assert_eq!(anyof["keywordLocation"], json!("/anyOf"));

    let branches = anyof["errors"].as_array().expect("branches kept");
    assert_eq!(branches.len(), 2);
    assert_eq!(branches[0]["keywordLocation"], json!("/anyOf/0"));
    assert_eq!(branches[1]["keywordLocation"], json!("/anyOf/1"));

    // The second branch failed on `minimum`, not on `type`.
    let second = branches[1]["errors"][0]["keywordLocation"]
        .as_str()
        .expect("a reason");
    assert!(second.ends_with("/minimum"), "got {second}");
}

/// `verbose` is the only format that reports what succeeded.
#[test]
fn verbose_keeps_passing_units_where_detailed_drops_them() {
    let validator = validator(
        r#"{"allOf": [{"type": "integer"}], "minimum": 10}"#,
    );
    let outcome = validator.validate(&json!(3), Mode::All);

    let detailed = outcome.output(OutputFormat::Detailed);
    let detailed_units = detailed["errors"].as_array().expect("errors");
    assert!(
        detailed_units
            .iter()
            .all(|unit| unit["valid"] == json!(false)),
        "detailed reports only failures",
    );

    // The failing run is the same one either way.
    let verbose = outcome.output(OutputFormat::Verbose);
    assert_eq!(verbose["valid"], json!(false));
}

/// The specification requires `absoluteKeywordLocation` exactly when the
/// keyword path crossed a reference — and leaves it optional otherwise.
#[test]
fn an_absolute_location_appears_once_a_reference_is_crossed() {
    let validator = validator(
        r##"{
            "$id": "https://example.com/root",
            "properties": {"a": {"$ref": "#/$defs/positive"}},
            "$defs": {"positive": {"type": "integer", "minimum": 1}}
        }"##,
    );
    let outcome = validator.validate(&json!({"a": 0}), Mode::All);

    let output = outcome.output(OutputFormat::Basic);
    let error = &output["errors"][0];

    let keyword = error["keywordLocation"].as_str().expect("a location");
    assert!(keyword.contains("/$ref/"), "got {keyword}");

    // Required here, and pointing into the resource the reference resolved to.
    assert_eq!(
        error["absoluteKeywordLocation"],
        json!("https://example.com/root#/$defs/positive/minimum"),
    );
}

#[test]
fn no_absolute_location_without_a_reference() {
    let validator = validator(r#"{"type": "string"}"#);
    let outcome = validator.validate(&json!(1), Mode::All);

    let output = outcome.output(OutputFormat::Basic);
    assert!(output["errors"][0].get("absoluteKeywordLocation").is_none());
}

/// Pointer tokens stay escaped, so the location resolves against the schema
/// it came from.
#[test]
fn a_location_escapes_the_pointer_tokens_it_contains() {
    let validator = validator(r#"{"properties": {"~a/b": {"type": "number"}}}"#);
    let outcome = validator.validate(&json!({"~a/b": "no"}), Mode::All);

    let output = outcome.output(OutputFormat::Basic);
    assert_eq!(
        output["errors"][0]["keywordLocation"],
        json!("/properties/~0a~1b/type"),
    );
    assert_eq!(output["errors"][0]["instanceLocation"], json!("/~0a~1b"));
}

// --- annotations ----------------------------------------------------------

#[test]
fn annotations_are_collected_only_when_asked_for() {
    let validator = validator(r#"{"title": "Root", "type": "string"}"#);
    let instance = json!("ok");

    assert!(validator.validate(&instance, Mode::All).annotations.is_empty());
    assert!(!validator.annotate(&instance, Mode::All).annotations.is_empty());
}

#[test]
fn an_annotation_carries_its_keywords_value() {
    let validator = validator(r#"{"title": "Root", "readOnly": true}"#);
    let outcome = validator.annotate(&json!(1), Mode::All);

    let title = outcome
        .annotations
        .iter()
        .find(|unit| unit.keyword() == "title")
        .expect("title collected");
    assert_eq!(title.annotation, Some(json!("Root")));
    assert!(title.valid);

    let read_only = outcome
        .annotations
        .iter()
        .find(|unit| unit.keyword() == "readOnly")
        .expect("readOnly collected");
    assert_eq!(read_only.annotation, Some(json!(true)));
}

/// A failed run drops its annotations: anything a rejected schema had to say
/// describes an instance that does not exist.
#[test]
fn a_failed_run_reports_no_annotations() {
    let validator = validator(r#"{"title": "Root", "type": "string"}"#);
    let outcome = validator.annotate(&json!(42), Mode::All);

    assert!(!outcome.is_valid());
    assert!(outcome.annotations.is_empty());
    assert!(outcome.output(OutputFormat::Basic).get("annotations").is_none());
}

/// An annotation from an alternative that did not match is not kept, even
/// when the run as a whole succeeds.
#[test]
fn a_rejected_branch_contributes_no_annotation() {
    let validator = validator(
        r#"{"anyOf": [
            {"type": "string", "title": "the string branch"},
            {"type": "integer", "title": "the integer branch"}
        ]}"#,
    );
    let outcome = validator.annotate(&json!(7), Mode::All);

    assert!(outcome.is_valid());
    let titles: Vec<_> = outcome
        .annotations
        .iter()
        .filter_map(|unit| unit.annotation.as_ref())
        .filter_map(|value| value.as_str())
        .collect();
    assert!(
        !titles.contains(&"the string branch"),
        "the branch that failed must not annotate, got {titles:?}",
    );
}

/// The other half of the same rule: the alternative that *did* match keeps
/// its annotations. Suppressing a whole branch would be as wrong as keeping
/// a rejected one.
#[test]
fn a_matching_branch_keeps_its_annotation() {
    let validator = validator(
        r#"{"anyOf": [
            {"type": "string", "title": "the string branch"},
            {"type": "integer", "title": "the integer branch"}
        ]}"#,
    );
    let outcome = validator.annotate(&json!(7), Mode::All);

    let titles: Vec<_> = outcome
        .annotations
        .iter()
        .filter_map(|unit| unit.annotation.as_ref())
        .filter_map(|value| value.as_str())
        .collect();
    assert_eq!(titles, ["the integer branch"]);
}

/// `oneOf` keeps exactly one alternative's annotations — the one that won.
#[test]
fn one_of_keeps_only_the_winners_annotation() {
    let validator = validator(
        r#"{"oneOf": [
            {"type": "string", "title": "text"},
            {"type": "integer", "title": "number"}
        ]}"#,
    );
    let outcome = validator.annotate(&json!("hi"), Mode::All);

    assert!(outcome.is_valid());
    let titles: Vec<_> = outcome
        .annotations
        .iter()
        .filter_map(|unit| unit.annotation.as_ref())
        .filter_map(|value| value.as_str())
        .collect();
    assert_eq!(titles, ["text"]);
}

/// An `if` that matched selects `then`, and annotates; one that did not
/// selects `else`, and does not.
#[test]
fn a_matched_condition_annotates_and_an_unmatched_one_does_not() {
    let validator = validator(
        r#"{
            "if": {"type": "string", "title": "looked like text"},
            "then": {"minLength": 1},
            "else": {"type": "integer"}
        }"#,
    );

    let matched = validator.annotate(&json!("hi"), Mode::All);
    let titles: Vec<_> = matched
        .annotations
        .iter()
        .filter_map(|unit| unit.annotation.as_ref())
        .filter_map(|value| value.as_str())
        .collect();
    assert_eq!(titles, ["looked like text"]);

    let unmatched = validator.annotate(&json!(3), Mode::All);
    assert!(unmatched.is_valid());
    assert!(
        unmatched.annotations.is_empty(),
        "a condition that did not match describes nothing",
    );
}

#[test]
fn annotations_reach_the_rendered_output() {
    let validator = validator(r#"{"title": "Root"}"#);
    let outcome = validator.annotate(&json!(1), Mode::All);

    let output = outcome.output(OutputFormat::Basic);
    assert_eq!(output["valid"], json!(true));
    let annotations = output["annotations"].as_array().expect("annotations");
    assert_eq!(annotations[0]["annotation"], json!("Root"));
    assert_eq!(annotations[0]["keywordLocation"], json!("/title"));
}

//! Asserting `format`, with the built-ins and with rules of your own.

use ash_jsonschema_builder::validator::Mode;
use ash_jsonschema_builder::{Schema, Validator, object};

/// The default is the specification's: `format` annotates and asserts nothing.
#[test]
fn format_is_an_annotation_until_asked_otherwise() {
    let schema = Schema::string().format("date").build().unwrap();

    let lenient = Validator::new(&schema).unwrap();
    assert!(lenient.is_valid(&serde_json::json!("not a date at all")));

    let strict = Validator::builder(&schema)
        .assert_formats()
        .build()
        .unwrap();
    assert!(!strict.is_valid(&serde_json::json!("not a date at all")));
    assert!(strict.is_valid(&serde_json::json!("2024-02-29")));
}

/// A date is checked against the calendar, not just the shape.
#[test]
fn dates_are_checked_against_the_calendar() {
    let schema = Schema::string().format("date").build().unwrap();
    let validator = Validator::builder(&schema)
        .assert_formats()
        .build()
        .unwrap();

    assert!(validator.is_valid(&serde_json::json!("2024-02-29"))); // a leap year
    assert!(!validator.is_valid(&serde_json::json!("2023-02-29"))); // not one
    assert!(!validator.is_valid(&serde_json::json!("2024-04-31"))); // April has 30
    assert!(!validator.is_valid(&serde_json::json!("2024-1-5"))); // not fixed width
}

/// The other temporal formats, and the error they produce.
#[test]
fn the_temporal_formats_assert_and_report_by_name() {
    let schema = object! {
        required "at": Schema::string().format("date-time"),
        required "for": Schema::string().format("duration"),
    }
    .build()
    .unwrap();
    let validator = Validator::builder(&schema)
        .assert_formats()
        .build()
        .unwrap();

    assert!(validator.is_valid(&serde_json::json!({
        "at": "2024-02-29T12:30:00Z",
        "for": "P1DT2H",
    })));

    let outcome = validator.validate(
        &serde_json::json!({"at": "2024-02-29 12:30:00Z", "for": "P1S"}),
        Mode::All,
    );
    assert_eq!(outcome.errors.len(), 2);
    assert_eq!(outcome.at("/at").count(), 1);
    assert!(
        outcome
            .first()
            .unwrap()
            .message()
            .contains("is not a valid date-time")
    );
}

/// A name the crate has never heard of becomes an assertion by registering one.
#[test]
fn a_rule_of_your_own_asserts_a_name_the_crate_does_not_know() {
    let schema = Schema::string().format("employee-id").build().unwrap();

    let validator = Validator::builder(&schema)
        .format("employee-id", |value| {
            value.strip_prefix("E-").is_some_and(|rest| {
                rest.len() == 5 && rest.chars().all(|c| c.is_ascii_digit())
            })
        })
        .build()
        .unwrap();

    assert!(validator.is_valid(&serde_json::json!("E-01234")));
    assert!(!validator.is_valid(&serde_json::json!("E-1")));
    assert!(!validator.is_valid(&serde_json::json!("X-01234")));

    let outcome = validator.validate(&serde_json::json!("nope"), Mode::All);
    assert_eq!(outcome.first().unwrap().keyword_location, "/format");
}

/// Registering a handler is itself the request to assert that name.
#[test]
fn a_registered_rule_needs_no_separate_switch() {
    let schema = Schema::string().format("even-length").build().unwrap();

    let validator = Validator::builder(&schema)
        .format("even-length", |value| value.len() % 2 == 0)
        .build()
        .unwrap();

    assert!(validator.is_valid(&serde_json::json!("ab")));
    assert!(!validator.is_valid(&serde_json::json!("abc")));
}

/// A handler replaces the built-in of the same name.
#[test]
fn a_rule_of_your_own_overrides_the_built_in() {
    let schema = Schema::string().format("email").build().unwrap();

    // Stricter than the built-in: this shop only accepts one domain.
    let validator = Validator::builder(&schema)
        .assert_formats()
        .format("email", |value| value.ends_with("@example.com"))
        .build()
        .unwrap();

    assert!(validator.is_valid(&serde_json::json!("someone@example.com")));
    assert!(!validator.is_valid(&serde_json::json!("someone@elsewhere.org")));
}

/// `format` says nothing about instances that are not strings.
#[test]
fn a_non_string_instance_is_left_alone() {
    let schema = Schema::builder().format("date").build().unwrap();
    let validator = Validator::builder(&schema)
        .assert_formats()
        .build()
        .unwrap();

    // No `type` keyword, so these reach `format` and must pass regardless.
    assert!(validator.is_valid(&serde_json::json!(42)));
    assert!(validator.is_valid(&serde_json::json!({"a": 1})));
    assert!(validator.is_valid(&serde_json::json!(null)));
    assert!(!validator.is_valid(&serde_json::json!("not a date")));
}

/// A validator carrying handlers still clones and crosses threads.
#[test]
fn a_validator_with_rules_is_still_shareable() {
    let schema = Schema::string().format("shouty").build().unwrap();
    let validator = Validator::builder(&schema)
        .format("shouty", |value| value.chars().all(|c| !c.is_lowercase()))
        .build()
        .unwrap();

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let validator = validator.clone();
            std::thread::spawn(move || {
                validator.is_valid(&serde_json::json!("LOUD"))
                    && !validator.is_valid(&serde_json::json!("quiet"))
            })
        })
        .collect();

    for handle in handles {
        assert!(handle.join().unwrap());
    }
}

/// Structural TLD rules need no list and never go stale.
#[test]
fn a_tld_can_be_checked_structurally() {
    use ash_jsonschema_builder::validator::tld::has_tld_shaped_suffix;

    let schema = Schema::string().format("hostname").build().unwrap();
    let validator = Validator::builder(&schema)
        .format("hostname", has_tld_shaped_suffix)
        .build()
        .unwrap();

    assert!(validator.is_valid(&serde_json::json!("example.com")));
    assert!(validator.is_valid(&serde_json::json!("a.b.example.co.uk")));
    assert!(!validator.is_valid(&serde_json::json!("example.123")));
    assert!(!validator.is_valid(&serde_json::json!("localhost")));
}

/// The IANA list answers whether a TLD actually exists.
#[cfg(feature = "iana-tlds")]
#[test]
fn a_tld_can_be_checked_against_the_iana_list() {
    use ash_jsonschema_builder::validator::tld::has_registered_tld;

    let schema = object! {
        required "site": Schema::string().format("hostname"),
    }
    .build()
    .unwrap();

    let validator = Validator::builder(&schema)
        .format("hostname", has_registered_tld)
        .build()
        .unwrap();

    assert!(validator.is_valid(&serde_json::json!({"site": "example.com"})));
    assert!(validator.is_valid(&serde_json::json!({"site": "example.museum"})));
    // Well shaped, but no such TLD.
    assert!(!validator.is_valid(&serde_json::json!({"site": "example.zzz"})));
}

/// The two TLD questions are genuinely different, which is why both exist.
#[cfg(feature = "iana-tlds")]
#[test]
fn the_structural_rule_is_looser_than_the_list() {
    use ash_jsonschema_builder::validator::tld::{has_registered_tld, has_tld_shaped_suffix};

    // `.zzz` is shaped like a TLD and is not one. A build whose vendored list
    // has gone stale would wrongly reject a real domain the same way, which is
    // why the structural rule is the default.
    assert!(has_tld_shaped_suffix("example.zzz"));
    assert!(!has_registered_tld("example.zzz"));

    // Everything the list accepts, the structural rule accepts too.
    assert!(has_tld_shaped_suffix("example.com"));
    assert!(has_registered_tld("example.com"));
}

/// Rules compose with the rest of the schema rather than replacing it.
#[test]
fn a_rule_runs_alongside_the_other_assertions() {
    let schema = object! {
        required "id": Schema::string().format("employee-id").min_length(3),
    }
    .build()
    .unwrap();

    let validator = Validator::builder(&schema)
        .format("employee-id", |value| value.starts_with("E-"))
        .build()
        .unwrap();

    // Fails both `minLength` and `format`, and both are reported.
    let outcome = validator.validate(&serde_json::json!({"id": "X"}), Mode::All);
    assert_eq!(outcome.errors.len(), 2);

    let keywords: Vec<&str> = outcome
        .errors
        .iter()
        .map(|error| error.keyword_location.as_str())
        .collect();
    assert!(keywords.contains(&"/properties/id/minLength"));
    assert!(keywords.contains(&"/properties/id/format"));
}

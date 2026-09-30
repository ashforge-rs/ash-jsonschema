//! What a validation run reports.
//!
//! The shape here is the one the specification describes, not one of this
//! crate's invention. 2020-12 defines four output formats — `flag`, `basic`,
//! `detailed`, and `verbose` — built from a single recursive *output unit*
//! carrying three locations and a verdict. [`Unit`] is that type, [`Outcome`]
//! is a run's root unit, and [`OutputFormat`] selects which of the four
//! [`Outcome::output`] renders.
//!
//! ## Why a tree, when errors read as a list
//!
//! A failed `anyOf` is one error at the top — "no alternative matched" — and
//! that is useless on its own, because the question a person actually has is
//! *which alternative came closest, and why did it fail?* The specification's
//! answer is nesting: the `anyOf` unit holds a child unit per branch, each with
//! its own errors. So the evaluator builds a tree.
//!
//! Most callers want a flat list of leaves, which is what [`Outcome::errors`]
//! stays. Both views come from the same run — the flat one is the tree with its
//! branches walked, not a second evaluation.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How much of the instance to check before reporting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Check everything and report every error.
    ///
    /// The cost of a fully invalid instance is proportional to how wrong it is,
    /// which is what you want when a person is going to read the result.
    #[default]
    All,
    /// Stop at the first error.
    ///
    /// Cheaper when the answer is only ever "did this pass?" — a hot path
    /// rejecting bad input, or a filter. The error reported is the first one
    /// found in document order, not necessarily the most important.
    FailFast,
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::All => "all",
            Self::FailFast => "fail-fast",
        })
    }
}

/// Which of the specification's four output formats to render.
///
/// They differ only in how much of the same run they show, so the choice is
/// about the reader, not about the validation: a machine that only branches on
/// pass/fail wants [`Flag`](OutputFormat::Flag), a person debugging a failed
/// `anyOf` wants [`Detailed`](OutputFormat::Detailed).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OutputFormat {
    /// `{"valid": false}` — the verdict, nothing else.
    Flag,
    /// The verdict plus one flat list of every error leaf.
    ///
    /// The common choice, and the default: it is the closest of the four to
    /// what [`Outcome::errors`] holds.
    #[default]
    Basic,
    /// The verdict plus the error tree, with branch structure kept.
    ///
    /// Units that neither failed nor add structure are collapsed away, so a
    /// failed `anyOf` keeps its per-branch children while a plain chain of
    /// passing keywords does not pad the output.
    Detailed,
    /// Every unit the run produced, passing ones included.
    ///
    /// Large, and the only format that shows what *succeeded* — which is what
    /// you want when the puzzle is why a schema matched rather than why it did
    /// not.
    Verbose,
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Flag => "flag",
            Self::Basic => "basic",
            Self::Detailed => "detailed",
            Self::Verbose => "verbose",
        })
    }
}

/// One node of a validation run: a verdict, where it came from, and why.
///
/// This is the specification's *output unit*. The three locations answer three
/// different questions, and a message that is actually useful needs all of
/// them:
///
/// - [`instance_location`](Unit::instance_location) — *where in your data*.
/// - [`keyword_location`](Unit::keyword_location) — *how the evaluation got
///   here*, including every `$ref` it followed, so the path is the one the
///   evaluator actually walked.
/// - [`absolute_keyword_location`](Unit::absolute_keyword_location) — *which
///   schema, resolved*. After a `$ref` the first two no longer say what
///   document you are in; this does.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unit {
    /// Whether this unit passed.
    pub valid: bool,
    /// A JSON Pointer into the *schema*, following `$ref`s as they were taken —
    /// `/properties/price/$ref/minimum`.
    #[serde(rename = "keywordLocation")]
    pub keyword_location: String,
    /// The absolute URI of the keyword, once references are resolved —
    /// `https://example.com/money#/minimum`.
    ///
    /// Present when the evaluation crossed a `$ref` or `$dynamicRef`, which is
    /// exactly when [`keyword_location`](Unit::keyword_location) stops being
    /// enough to find the keyword. The specification requires it in that case
    /// and leaves it optional otherwise.
    #[serde(
        rename = "absoluteKeywordLocation",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub absolute_keyword_location: Option<String>,
    /// A JSON Pointer into the *instance* — `/items/2/price`. The empty string
    /// is the whole instance.
    #[serde(rename = "instanceLocation")]
    pub instance_location: String,
    /// What went wrong, in one line. Absent on a unit that passed.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
    /// The value a successful annotation keyword produced.
    ///
    /// `title`, `description`, `default`, `readOnly` and their kind assert
    /// nothing — they attach information to the instance location. This is
    /// that information, and its presence is what makes a unit an annotation
    /// rather than an error.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub annotation: Option<Value>,
    /// Units nested beneath this one — the branches of a failed `anyOf`, the
    /// subschemas of an `allOf`.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub errors: Vec<Unit>,
}

impl Unit {
    /// The keyword that produced this unit — the last segment of its location.
    ///
    /// ```
    /// # use ash_jsonschema::validator::Unit;
    /// let unit = Unit {
    ///     keyword_location: "/properties/age/minimum".to_owned(),
    ///     ..Unit::default()
    /// };
    /// assert_eq!(unit.keyword(), "minimum");
    /// ```
    pub fn keyword(&self) -> &str {
        self.keyword_location
            .rsplit('/')
            .next()
            .unwrap_or(&self.keyword_location)
    }

    /// This unit's message, or the empty string when it has none.
    ///
    /// A passing unit carries no message, and neither does one that exists
    /// only to hold the units beneath it.
    ///
    /// ```
    /// # use ash_jsonschema::validator::Unit;
    /// let unit = Unit { error: Some("must be a string".to_owned()), ..Unit::default() };
    /// assert_eq!(unit.message(), "must be a string");
    /// assert_eq!(Unit::default().message(), "");
    /// ```
    pub fn message(&self) -> &str {
        self.error.as_deref().unwrap_or_default()
    }

    /// Whether this unit or anything under it failed.
    fn has_failure(&self) -> bool {
        !self.valid
    }

    /// Walks this unit and everything beneath it, outermost first.
    fn visit<'a>(&'a self, seen: &mut Vec<&'a Unit>) {
        seen.push(self);
        for child in &self.errors {
            child.visit(seen);
        }
    }

    /// The failing units with no failing children — the leaves that carry the
    /// actual messages.
    fn leaves<'a>(&'a self, into: &mut Vec<&'a Unit>) {
        if self.valid {
            return;
        }
        let mut any = false;
        for child in &self.errors {
            if child.has_failure() {
                any = true;
                child.leaves(into);
            }
        }
        if !any && self.error.is_some() {
            into.push(self);
        }
    }

    /// This unit rendered in one of the specification's formats.
    fn render(&self, format: OutputFormat) -> Value {
        let mut object = serde_json::Map::new();
        object.insert("valid".into(), Value::Bool(self.valid));
        object.insert(
            "keywordLocation".into(),
            Value::String(self.keyword_location.clone()),
        );
        if let Some(absolute) = &self.absolute_keyword_location {
            object.insert(
                "absoluteKeywordLocation".into(),
                Value::String(absolute.clone()),
            );
        }
        object.insert(
            "instanceLocation".into(),
            Value::String(self.instance_location.clone()),
        );
        if let Some(error) = &self.error {
            object.insert("error".into(), Value::String(error.clone()));
        }
        if let Some(annotation) = &self.annotation {
            object.insert("annotation".into(), annotation.clone());
        }

        let children: Vec<Value> = match format {
            OutputFormat::Verbose => self.errors.iter().map(|c| c.render(format)).collect(),
            _ => self
                .errors
                .iter()
                .filter(|c| c.has_failure())
                .map(|c| c.render(format))
                .collect(),
        };
        if !children.is_empty() {
            object.insert("errors".into(), Value::Array(children));
        }
        Value::Object(object)
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let location = match self.instance_location.is_empty() {
            true => "(root)",
            false => &self.instance_location,
        };
        let message = self.error.as_deref().unwrap_or("is valid");
        write!(f, "{location}: {message} [{}]", self.keyword_location)
    }
}

/// One way an instance failed its schema.
///
/// A failing [`Unit`] — the two are the same type, and this name is what the
/// flat view calls it.
pub type ValidationError = Unit;

/// The result of checking one instance.
///
/// An outcome is produced whether or not the instance passed;
/// [`is_valid`](Outcome::is_valid) is the question, and the units are the
/// explanation. It holds the run's whole tree; [`errors`](Outcome::errors) is
/// the flat view of it that most callers want.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The run's root unit, with every nested unit beneath it.
    ///
    /// Boxed to keep `Outcome` small: it is returned as the `Err` of
    /// [`check`](crate::Validator::check), and an error type that has to be
    /// memcpy'd through every `?` is a cost paid on the path where nothing
    /// went wrong.
    root: Box<Unit>,
    /// The failing leaves, in document order. Empty when the instance is valid.
    ///
    /// Flattened once when the outcome is built, since almost every caller
    /// reads it and walking the tree per access would be the wasteful way
    /// round.
    pub errors: Vec<ValidationError>,
    /// The annotations a successful run produced, in document order.
    ///
    /// Empty when the instance failed: the specification drops the
    /// annotations of any schema that did not match, since an annotation from
    /// a rejected branch describes nothing the instance actually is.
    pub annotations: Vec<Unit>,
}

impl Default for Outcome {
    fn default() -> Self {
        Self::valid()
    }
}

impl Outcome {
    /// An outcome for an instance that passed.
    pub(crate) fn valid() -> Self {
        Self::from_parts(
            Unit {
                valid: true,
                ..Unit::default()
            },
            Vec::new(),
        )
    }

    /// Builds an outcome from a root unit and the annotations the run
    /// collected, flattening the tree once.
    pub(crate) fn from_parts(root: Unit, annotations: Vec<Unit>) -> Self {
        let mut leaves = Vec::new();
        root.leaves(&mut leaves);
        let errors = leaves.into_iter().cloned().collect();
        // A run that failed produces no annotations: anything a rejected
        // schema had to say describes an instance that does not exist.
        let annotations = match root.valid {
            true => annotations,
            false => Vec::new(),
        };
        Self {
            root: Box::new(root),
            errors,
            annotations,
        }
    }

    /// Whether the instance satisfied the schema.
    pub fn is_valid(&self) -> bool {
        self.root.valid
    }

    /// The first error, if any.
    ///
    /// Under [`Mode::FailFast`] this is the only one there is.
    pub fn first(&self) -> Option<&ValidationError> {
        self.errors.first()
    }

    /// The run's whole tree, root first.
    ///
    /// Where [`errors`](Outcome::errors) gives the leaves, this keeps the
    /// structure that led to them — which branch of an `anyOf` a failure came
    /// from, and what it was nested under.
    ///
    /// ```
    /// # use ash_jsonschema::{Schema, Validator};
    /// # use ash_jsonschema::validator::Mode;
    /// let schema = Schema::parse(r#"{
    ///     "anyOf": [{"type": "string"}, {"type": "integer", "minimum": 10}]
    /// }"#)?;
    ///
    /// let validator = Validator::new(&schema)?;
    /// let outcome = validator.validate(&serde_json::json!(3), Mode::All);
    ///
    /// // The flat view names the branches that failed.
    /// assert!(!outcome.is_valid());
    /// assert!(outcome.errors.len() >= 1);
    ///
    /// // The tree says which alternative each failure came from.
    /// let anyof = outcome.unit();
    /// assert!(!anyof.valid);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn unit(&self) -> &Unit {
        &self.root
    }

    /// Every unit the run produced, outermost first.
    pub fn units(&self) -> Vec<&Unit> {
        let mut seen = Vec::new();
        self.root.visit(&mut seen);
        seen
    }

    /// This run rendered in one of the specification's output formats.
    ///
    /// The result is JSON that satisfies
    /// `https://json-schema.org/draft/2020-12/output/schema`, so it can cross a
    /// process boundary and be read by tooling that has never heard of this
    /// crate.
    ///
    /// ```
    /// # use ash_jsonschema::{Schema, Validator};
    /// # use ash_jsonschema::validator::{Mode, OutputFormat};
    /// let schema = Schema::parse(r#"{"type": "string"}"#)?;
    /// let validator = Validator::new(&schema)?;
    /// let outcome = validator.validate(&serde_json::json!(42), Mode::All);
    ///
    /// let flag = outcome.output(OutputFormat::Flag);
    /// assert_eq!(flag, serde_json::json!({"valid": false}));
    ///
    /// let basic = outcome.output(OutputFormat::Basic);
    /// assert_eq!(basic["valid"], serde_json::json!(false));
    /// assert!(basic["errors"].as_array().is_some_and(|e| !e.is_empty()));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn output(&self, format: OutputFormat) -> Value {
        match format {
            OutputFormat::Flag => serde_json::json!({"valid": self.is_valid()}),
            OutputFormat::Basic => {
                let mut object = serde_json::Map::new();
                object.insert("valid".into(), Value::Bool(self.is_valid()));
                if !self.errors.is_empty() {
                    let errors: Vec<Value> = self
                        .errors
                        .iter()
                        .map(|unit| {
                            let mut flat = unit.clone();
                            flat.errors.clear();
                            flat.render(format)
                        })
                        .collect();
                    object.insert("errors".into(), Value::Array(errors));
                }
                if !self.annotations.is_empty() {
                    let annotations: Vec<Value> = self
                        .annotations
                        .iter()
                        .map(|unit| unit.render(format))
                        .collect();
                    object.insert("annotations".into(), Value::Array(annotations));
                }
                Value::Object(object)
            }
            OutputFormat::Detailed | OutputFormat::Verbose => {
                let mut rendered = self.root.render(format);
                if !self.annotations.is_empty()
                    && let Some(object) = rendered.as_object_mut()
                {
                    let annotations: Vec<Value> = self
                        .annotations
                        .iter()
                        .map(|unit| unit.render(format))
                        .collect();
                    object.insert("annotations".into(), Value::Array(annotations));
                }
                rendered
            }
        }
    }

    /// Turns the outcome into a `Result`, for a caller that would rather use
    /// `?` than branch.
    ///
    /// ```
    /// # use ash_jsonschema::{Schema, Validator};
    /// # use ash_jsonschema::validator::Mode;
    /// let validator = Validator::new(&Schema::string().build()?)?;
    ///
    /// validator.validate(&serde_json::json!("ok"), Mode::All).into_result()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// The outcome itself, when the instance did not pass.
    pub fn into_result(self) -> Result<(), Outcome> {
        match self.is_valid() {
            true => Ok(()),
            false => Err(self),
        }
    }

    /// The errors at, or beneath, one place in the instance.
    ///
    /// ```
    /// # use ash_jsonschema::{Schema, Validator, object};
    /// # use ash_jsonschema::validator::Mode;
    /// let schema = object! {
    ///     required "name": Schema::string().min_length(1),
    ///     required "age": Schema::integer().minimum(0),
    /// }.build()?;
    ///
    /// let validator = Validator::new(&schema)?;
    /// let outcome = validator.validate(
    ///     &serde_json::json!({"name": "", "age": -1}),
    ///     Mode::All,
    /// );
    ///
    /// assert_eq!(outcome.at("/name").count(), 1);
    /// assert_eq!(outcome.at("/age").count(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn at<'a>(&'a self, location: &'a str) -> impl Iterator<Item = &'a ValidationError> {
        self.errors.iter().filter(move |error| {
            error.instance_location == location
                || error
                    .instance_location
                    .strip_prefix(location)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_valid() {
            return f.write_str("the instance is valid");
        }
        let count = self.errors.len();
        let plural = match count {
            1 => "error",
            _ => "errors",
        };
        write!(f, "the instance is not valid ({count} {plural})")?;
        for error in &self.errors {
            write!(f, "\n  - {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Outcome {}

/// Renders a value on one line, truncated, for an error message.
pub(crate) fn one_line(value: &Value) -> String {
    const LIMIT: usize = 48;

    let text = value.to_string();
    match text.char_indices().nth(LIMIT) {
        Some((index, _)) => format!("{}…", &text[..index]),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(location: &str) -> Unit {
        Unit {
            valid: false,
            keyword_location: "/type".to_owned(),
            absolute_keyword_location: None,
            instance_location: location.to_owned(),
            error: Some("wrong".to_owned()),
            annotation: None,
            errors: Vec::new(),
        }
    }

    fn failing(children: Vec<Unit>) -> Outcome {
        Outcome::from_parts(
            Unit {
                valid: false,
                keyword_location: String::new(),
                absolute_keyword_location: None,
                instance_location: String::new(),
                error: Some("does not match".to_owned()),
                annotation: None,
                errors: children,
            },
            Vec::new(),
        )
    }

    #[test]
    fn a_valid_outcome_holds_no_errors() {
        let outcome = Outcome::valid();
        assert!(outcome.is_valid());
        assert_eq!(outcome.first(), None);
        assert_eq!(outcome.to_string(), "the instance is valid");
        assert!(outcome.into_result().is_ok());
    }

    #[test]
    fn display_names_the_root_readably_and_lists_every_error() {
        let outcome = failing(vec![error(""), error("/a")]);

        let text = outcome.to_string();
        assert!(text.starts_with("the instance is not valid (2 errors)"));
        assert!(text.contains("(root): wrong [/type]"));
        assert!(text.contains("/a: wrong [/type]"));
    }

    #[test]
    fn at_matches_a_location_and_everything_under_it() {
        let outcome = failing(vec![error("/a"), error("/a/b"), error("/ab"), error("/c")]);

        // `/a` and `/a/b`, but not `/ab` — a prefix must end at a segment.
        assert_eq!(outcome.at("/a").count(), 2);
        assert_eq!(outcome.at("/c").count(), 1);
        assert_eq!(outcome.at("/nope").count(), 0);
    }

    #[test]
    fn into_result_carries_the_outcome_as_the_error() {
        let outcome = failing(vec![error("/a")]);
        let error = outcome.clone().into_result().unwrap_err();
        assert_eq!(error, outcome);
    }

    #[test]
    fn a_long_value_is_truncated_for_display() {
        let short = serde_json::json!({"a": 1});
        assert_eq!(one_line(&short), r#"{"a":1}"#);

        let long = serde_json::json!({"key": "x".repeat(200)});
        let rendered = one_line(&long);
        assert!(rendered.ends_with('…'));
        assert!(rendered.chars().count() <= 50);
    }

    /// The flat view keeps only leaves: a parent that merely nests failing
    /// children is structure, not a message a person needs twice.
    #[test]
    fn flattening_keeps_the_leaves_and_drops_the_scaffolding() {
        let outcome = failing(vec![Unit {
            valid: false,
            keyword_location: "/anyOf".to_owned(),
            absolute_keyword_location: None,
            instance_location: String::new(),
            error: Some("no alternative matched".to_owned()),
            annotation: None,
            errors: vec![error("/a"), error("/b")],
        }]);

        // Two leaves, not the two scaffolding units above them.
        assert_eq!(outcome.errors.len(), 2);
        assert!(outcome.errors.iter().all(|e| e.errors.is_empty()));

        // The tree still holds everything.
        assert_eq!(outcome.units().len(), 4);
    }

    /// A failing unit with no children of its own is itself a leaf.
    #[test]
    fn a_childless_failure_is_its_own_leaf() {
        let outcome = failing(vec![]);
        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(outcome.errors[0].error.as_deref(), Some("does not match"));
    }

    #[test]
    fn keyword_is_the_last_segment_of_the_location() {
        let unit = Unit {
            keyword_location: "/properties/age/minimum".to_owned(),
            ..Unit::default()
        };
        assert_eq!(unit.keyword(), "minimum");

        let root = Unit::default();
        assert_eq!(root.keyword(), "");
    }

    #[test]
    fn flag_output_is_only_the_verdict() {
        assert_eq!(
            Outcome::valid().output(OutputFormat::Flag),
            serde_json::json!({"valid": true}),
        );
        assert_eq!(
            failing(vec![error("/a")]).output(OutputFormat::Flag),
            serde_json::json!({"valid": false}),
        );
    }

    #[test]
    fn basic_output_is_flat() {
        let outcome = failing(vec![Unit {
            valid: false,
            keyword_location: "/anyOf".to_owned(),
            absolute_keyword_location: None,
            instance_location: String::new(),
            error: Some("no alternative matched".to_owned()),
            annotation: None,
            errors: vec![error("/a")],
        }]);

        let output = outcome.output(OutputFormat::Basic);
        let errors = output["errors"].as_array().unwrap();

        // One entry per leaf, and no nesting inside any of them.
        assert_eq!(errors.len(), 1);
        assert!(errors.iter().all(|e| e.get("errors").is_none()));
    }

    #[test]
    fn detailed_output_keeps_the_nesting() {
        let outcome = failing(vec![Unit {
            valid: false,
            keyword_location: "/anyOf".to_owned(),
            absolute_keyword_location: None,
            instance_location: String::new(),
            error: Some("no alternative matched".to_owned()),
            annotation: None,
            errors: vec![error("/a")],
        }]);

        let output = outcome.output(OutputFormat::Detailed);
        let anyof = &output["errors"][0];
        assert_eq!(anyof["keywordLocation"], serde_json::json!("/anyOf"));
        assert!(anyof["errors"].as_array().is_some_and(|e| e.len() == 1));
    }

    /// `detailed` drops units that passed; `verbose` is the only format that
    /// keeps them.
    #[test]
    fn verbose_keeps_passing_units_and_detailed_does_not() {
        let passing = Unit {
            valid: true,
            keyword_location: "/type".to_owned(),
            absolute_keyword_location: None,
            instance_location: String::new(),
            error: None,
            annotation: None,
            errors: Vec::new(),
        };
        let outcome = failing(vec![passing, error("/a")]);

        let detailed = outcome.output(OutputFormat::Detailed);
        assert_eq!(detailed["errors"].as_array().unwrap().len(), 1);

        let verbose = outcome.output(OutputFormat::Verbose);
        assert_eq!(verbose["errors"].as_array().unwrap().len(), 2);
    }

    /// The specification requires `absoluteKeywordLocation` when the keyword
    /// path crossed a reference, and leaves it out otherwise.
    #[test]
    fn an_absolute_location_is_written_only_when_present() {
        let mut unit = error("/a");
        assert!(
            unit.render(OutputFormat::Basic)
                .get("absoluteKeywordLocation")
                .is_none()
        );

        unit.absolute_keyword_location = Some("https://example.com/s#/type".to_owned());
        let rendered = unit.render(OutputFormat::Basic);
        assert_eq!(
            rendered["absoluteKeywordLocation"],
            serde_json::json!("https://example.com/s#/type"),
        );
    }
}

//! What a validation run reports.

use std::fmt;

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

/// One way an instance failed its schema.
///
/// The two pointers answer different questions, and a useful message needs
/// both: [`location`](ValidationError::location) is *where in your data* the
/// problem is, and [`keyword`](ValidationError::keyword) is *which rule in the
/// schema* it broke.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationError {
    /// A JSON Pointer into the *instance* — `/items/2/price` points at the
    /// offending value. The empty string is the whole instance.
    pub location: String,
    /// A JSON Pointer into the *schema* — `/properties/price/minimum` names the
    /// keyword that rejected it.
    pub keyword: String,
    /// What went wrong, in one line.
    pub message: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let location = match self.location.is_empty() {
            true => "(root)",
            false => &self.location,
        };
        write!(f, "{location}: {} [{}]", self.message, self.keyword)
    }
}

/// The result of checking one instance.
///
/// An outcome is produced whether or not the instance passed;
/// [`is_valid`](Outcome::is_valid) is the question, and the errors are the
/// explanation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Every error found, in document order. Empty when the instance is valid.
    ///
    /// Under [`Mode::FailFast`] this holds at most one.
    pub errors: Vec<ValidationError>,
}

impl Outcome {
    /// Whether the instance satisfied the schema.
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// The first error, if any.
    ///
    /// Under [`Mode::FailFast`] this is the only one there is.
    pub fn first(&self) -> Option<&ValidationError> {
        self.errors.first()
    }

    /// Turns the outcome into a `Result`, for a caller that would rather use
    /// `?` than branch.
    ///
    /// ```
    /// # use ash_jsonschema_builder::{Schema, Validator};
    /// # use ash_jsonschema_builder::validator::Mode;
    /// let validator = Validator::new(&Schema::string().build()?)?;
    ///
    /// validator.validate(&serde_json::json!("ok"), Mode::All).into_result()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// The outcome itself, when it holds any error.
    pub fn into_result(self) -> Result<(), Outcome> {
        match self.is_valid() {
            true => Ok(()),
            false => Err(self),
        }
    }

    /// The errors at, or beneath, one place in the instance.
    ///
    /// ```
    /// # use ash_jsonschema_builder::{Schema, Validator, object};
    /// # use ash_jsonschema_builder::validator::Mode;
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
            error.location == location
                || error
                    .location
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

    fn error(location: &str) -> ValidationError {
        ValidationError {
            location: location.to_owned(),
            keyword: "/type".to_owned(),
            message: "wrong".to_owned(),
        }
    }

    #[test]
    fn a_valid_outcome_holds_no_errors() {
        let outcome = Outcome::default();
        assert!(outcome.is_valid());
        assert_eq!(outcome.first(), None);
        assert_eq!(outcome.to_string(), "the instance is valid");
        assert!(outcome.into_result().is_ok());
    }

    #[test]
    fn display_names_the_root_readably_and_lists_every_error() {
        let outcome = Outcome {
            errors: vec![error(""), error("/a")],
        };

        let text = outcome.to_string();
        assert!(text.starts_with("the instance is not valid (2 errors)"));
        assert!(text.contains("(root): wrong [/type]"));
        assert!(text.contains("/a: wrong [/type]"));
    }

    #[test]
    fn at_matches_a_location_and_everything_under_it() {
        let outcome = Outcome {
            errors: vec![error("/a"), error("/a/b"), error("/ab"), error("/c")],
        };

        // `/a` and `/a/b`, but not `/ab` — a prefix must end at a segment.
        assert_eq!(outcome.at("/a").count(), 2);
        assert_eq!(outcome.at("/c").count(), 1);
        assert_eq!(outcome.at("/nope").count(), 0);
    }

    #[test]
    fn into_result_carries_the_outcome_as_the_error() {
        let outcome = Outcome {
            errors: vec![error("/a")],
        };
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
}

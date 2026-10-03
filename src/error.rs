//! What meta-schema validation reports when a built schema is not a legal
//! JSON Schema 2020-12 document.

use std::fmt;

/// One way in which a schema failed the meta-schema.
///
/// The two locations are JSON Pointers, and together they say both *where* in
/// your document the problem is and *which rule* it broke.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// A JSON Pointer into the schema being checked — `/properties/id/$anchor`
    /// points at the `$anchor` keyword of the `id` property's subschema. The
    /// empty string is the root.
    pub location: String,
    /// A JSON Pointer into the meta-schema, naming the rule that failed —
    /// `/allOf/0/properties/$anchor/pattern`.
    pub rule: String,
    /// What went wrong, in one line.
    pub message: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let location = if self.location.is_empty() {
            "(root)"
        } else {
            &self.location
        };
        write!(f, "{location}: {} [{}]", self.message, self.rule)
    }
}

/// One or more ways a schema failed a check.
///
/// Always holds at least one [`Violation`], in the order the walk found them:
/// outermost keyword first, then down through the subschemas.
///
/// Two checks report through this type, and they say different things, so the
/// error carries which one ran: [`Builder::build`](crate::Builder::build)
/// checks a document's *shape* against the meta-schema, while
/// [`Registry::check_references`](crate::Registry::check_references) checks
/// that its `$ref`s land somewhere. A dangling reference is a perfectly valid
/// 2020-12 document, so it must not be reported as a malformed one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaError {
    violations: Vec<Violation>,
    kind: ErrorKind,
}

/// Which check produced a [`SchemaError`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The document is not a well-formed JSON Schema 2020-12.
    MetaSchema,
    /// The document is well-formed, but a reference in it resolves to nothing.
    References,
}

impl ErrorKind {
    /// The headline this kind of failure is reported under.
    fn headline(&self) -> &'static str {
        match self {
            Self::MetaSchema => "the built schema is not valid JSON Schema 2020-12",
            Self::References => "the schema has references that resolve to nothing",
        }
    }
}

impl SchemaError {
    /// Wraps a non-empty list of violations.
    ///
    /// # Panics
    ///
    /// If `violations` is empty — an error with nothing wrong is a bug.
    pub(crate) fn new(violations: Vec<Violation>) -> Self {
        Self::of_kind(violations, ErrorKind::MetaSchema)
    }

    /// Wraps a non-empty list of violations from a named check.
    ///
    /// # Panics
    ///
    /// If `violations` is empty — an error with nothing wrong is a bug.
    pub(crate) fn of_kind(violations: Vec<Violation>, kind: ErrorKind) -> Self {
        assert!(
            !violations.is_empty(),
            "a SchemaError must carry at least one violation"
        );
        Self { violations, kind }
    }

    /// Which check reported this.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Every violation found, in walk order.
    pub fn violations(&self) -> &[Violation] {
        &self.violations
    }

    /// The first violation. Never `None`.
    pub fn first(&self) -> &Violation {
        &self.violations[0]
    }
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.violations.len();
        let plural = if count == 1 {
            "violation"
        } else {
            "violations"
        };
        write!(f, "{} ({count} {plural})", self.kind.headline())?;
        for violation in &self.violations {
            write!(f, "\n  - {violation}")?;
        }
        Ok(())
    }
}

impl std::error::Error for SchemaError {}

/// What [`Schema::parse_checked`](crate::Schema::parse_checked) reports.
///
/// The two cases are kept apart because they mean different things to a
/// caller: [`Syntax`](ParseError::Syntax) says the text was not JSON, while
/// [`Invalid`](ParseError::Invalid) says it parsed cleanly and then failed the
/// meta-schema.
#[derive(Debug)]
#[non_exhaustive]
pub enum ParseError {
    /// The text was not well-formed JSON.
    Syntax(serde_json::Error),
    /// The document parsed but is not a valid JSON Schema 2020-12.
    Invalid(SchemaError),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(error) => write!(f, "the schema is not well-formed JSON: {error}"),
            Self::Invalid(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Syntax(error) => Some(error),
            Self::Invalid(error) => Some(error),
        }
    }
}

impl From<serde_json::Error> for ParseError {
    fn from(value: serde_json::Error) -> Self {
        Self::Syntax(value)
    }
}

impl From<SchemaError> for ParseError {
    fn from(value: SchemaError) -> Self {
        Self::Invalid(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn violation(location: &str) -> Violation {
        Violation {
            location: location.to_owned(),
            rule: "/properties/x".to_owned(),
            message: "nope".to_owned(),
        }
    }

    #[test]
    fn display_names_the_root_readably() {
        let error = SchemaError::new(vec![violation("")]);
        assert_eq!(
            error.to_string(),
            "the built schema is not valid JSON Schema 2020-12 (1 violation)\n  - (root): nope [/properties/x]",
        );
    }

    #[test]
    fn display_lists_every_violation() {
        let error = SchemaError::new(vec![violation("/a"), violation("/b")]);
        assert_eq!(error.violations().len(), 2);
        assert_eq!(error.first().location, "/a");
        assert!(error.to_string().starts_with(
            "the built schema is not valid JSON Schema 2020-12 (2 violations)\n  - /a: "
        ));
    }

    #[test]
    #[should_panic(expected = "at least one violation")]
    fn an_empty_error_is_a_bug() {
        let _ = SchemaError::new(Vec::new());
    }
}

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

/// A built schema that the 2020-12 meta-schema rejected.
///
/// Always holds at least one [`Violation`], in the order the walk found them:
/// outermost keyword first, then down through the subschemas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaError {
    violations: Vec<Violation>,
}

impl SchemaError {
    /// Wraps a non-empty list of violations.
    ///
    /// # Panics
    ///
    /// If `violations` is empty — an error with nothing wrong is a bug.
    pub(crate) fn new(violations: Vec<Violation>) -> Self {
        assert!(
            !violations.is_empty(),
            "a SchemaError must carry at least one violation"
        );
        Self { violations }
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
        write!(
            f,
            "the built schema is not valid JSON Schema 2020-12 ({count} {plural})"
        )?;
        for violation in &self.violations {
            write!(f, "\n  - {violation}")?;
        }
        Ok(())
    }
}

impl std::error::Error for SchemaError {}

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

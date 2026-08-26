//! Checking that an instance satisfies a schema.
//!
//! The rest of the crate is about schemas as documents — building them, reading
//! them, comparing them. This is the other half: given a schema, does a
//! particular piece of JSON satisfy it?
//!
//! ```
//! # use ash_jsonschema_builder::{Schema, Validator, object};
//! # use ash_jsonschema_builder::validator::Mode;
//! let schema = object! {
//!     required "id": Schema::string().min_length(1),
//!     required "age": Schema::integer().minimum(0),
//! }
//! .closed()
//! .build()?;
//!
//! let validator = Validator::new(&schema)?;
//!
//! assert!(validator.is_valid(&serde_json::json!({"id": "a", "age": 30})));
//!
//! let outcome = validator.validate(
//!     &serde_json::json!({"id": "", "age": -1, "extra": true}),
//!     Mode::All,
//! );
//! assert_eq!(outcome.errors.len(), 3);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## The schema is fixed
//!
//! A [`Validator`] holds its compiled schema and offers no way to change it.
//! That is the point: a validator is built once, checked once, and then only
//! answers questions. To validate against a different schema, make a different
//! validator — a schema that could change under a running check is a schema
//! nobody can reason about.
//!
//! Compiling is where a *schema* can be wrong: every `pattern` is compiled and
//! every reference indexed up front, so a validator that exists is one whose
//! schema was understood, and [`validate`](Validator::validate) has no failure
//! mode of its own.
//!
//! ## Two modes
//!
//! [`Mode::All`] reports every error, which is what a person reading the result
//! wants. [`Mode::FailFast`] stops at the first, which is cheaper when the only
//! question is whether the instance passed.
//!
//! ## Coverage
//!
//! Every assertion and applicator of the 2020-12 dialect, including
//! `unevaluatedProperties` and `unevaluatedItems` with the annotation
//! collection they require, and `$dynamicRef` / `$dynamicAnchor`.
//!
//! Two things are deliberately not asserted. `format` is an annotation by
//! default in 2020-12 and is not checked. The content vocabulary
//! (`contentEncoding`, `contentMediaType`, `contentSchema`) is likewise an
//! annotation and is not decoded.

mod compile;
mod eval;
mod outcome;

use std::sync::Arc;

use serde_json::Value;

pub use compile::CompileError;
pub use outcome::{Mode, Outcome, ValidationError};

use crate::registry::Registry;
use crate::schema::Schema;
use compile::Compiled;

/// A schema, compiled and ready to check instances against.
///
/// Cloning is cheap and shares the compiled schema, and the result is
/// `Send + Sync`, so one validator built at startup serves as many threads as
/// you like.
///
/// ```
/// # use ash_jsonschema_builder::{Schema, Validator};
/// let validator = Validator::new(&Schema::string().min_length(2).build()?)?;
///
/// assert!(validator.is_valid(&serde_json::json!("ok")));
/// assert!(!validator.is_valid(&serde_json::json!("x")));
/// assert!(!validator.is_valid(&serde_json::json!(42)));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug)]
pub struct Validator {
    compiled: Arc<Compiled>,
}

impl Validator {
    /// Compiles `schema` into a validator.
    ///
    /// References are resolved within the schema itself. Use
    /// [`with_registry`](Self::with_registry) for a schema whose `$ref`s reach
    /// other documents.
    ///
    /// # Errors
    ///
    /// [`CompileError::BadPattern`] if a `pattern` will not compile, and
    /// [`CompileError::NotASchema`] if the document is not a schema at all.
    pub fn new(schema: &Schema) -> Result<Self, CompileError> {
        Self::build(Arc::new(schema.to_value()), None)
    }

    /// Compiles `schema`, resolving its references through `registry`.
    ///
    /// Everything the registry holds is indexed into the validator, so it is
    /// not borrowed afterwards — the validator stands alone once built.
    ///
    /// ```
    /// # use ash_jsonschema_builder::{Registry, Schema, Validator};
    /// let address = Schema::object()
    ///     .id("https://example.com/address")
    ///     .required_property("city", Schema::string())
    ///     .build()?;
    /// let user = Schema::object()
    ///     .id("https://example.com/user")
    ///     .required_property("address", Schema::reference("address"))
    ///     .build()?;
    ///
    /// let mut registry = Registry::new();
    /// registry.insert(&address)?;
    /// registry.insert(&user)?;
    ///
    /// let validator = Validator::with_registry(&user, &registry)?;
    ///
    /// assert!(validator.is_valid(&serde_json::json!({"address": {"city": "Athens"}})));
    /// assert!(!validator.is_valid(&serde_json::json!({"address": {}})));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_registry(schema: &Schema, registry: &Registry) -> Result<Self, CompileError> {
        Self::build(Arc::new(schema.to_value()), Some(registry))
    }

    /// Compiles a document that did not come from this crate's builders.
    ///
    /// Nothing checks that it is a well-formed schema beyond what compiling
    /// needs; pair it with [`validate_document`](crate::validate_document) when
    /// the source is untrusted.
    pub fn from_document(document: &Value) -> Result<Self, CompileError> {
        Self::build(Arc::new(document.clone()), None)
    }

    fn build(document: Arc<Value>, registry: Option<&Registry>) -> Result<Self, CompileError> {
        Ok(Self {
            compiled: Arc::new(Compiled::new(document, registry)?),
        })
    }

    /// Checks `instance`, reporting according to `mode`.
    ///
    /// ```
    /// # use ash_jsonschema_builder::{Schema, Validator, object};
    /// # use ash_jsonschema_builder::validator::Mode;
    /// let schema = object! {
    ///     required "a": Schema::string(),
    ///     required "b": Schema::string(),
    /// }.build()?;
    /// let validator = Validator::new(&schema)?;
    /// let instance = serde_json::json!({});
    ///
    /// assert_eq!(validator.validate(&instance, Mode::All).errors.len(), 2);
    /// assert_eq!(validator.validate(&instance, Mode::FailFast).errors.len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn validate(&self, instance: &Value, mode: Mode) -> Outcome {
        eval::run(&self.compiled, instance, mode)
    }

    /// Whether `instance` satisfies the schema, stopping at the first problem.
    ///
    /// Shorthand for `validate(instance, Mode::FailFast).is_valid()`.
    pub fn is_valid(&self, instance: &Value) -> bool {
        self.validate(instance, Mode::FailFast).is_valid()
    }

    /// Checks `instance` and returns the outcome as a `Result`, reporting every
    /// error.
    ///
    /// # Errors
    ///
    /// The [`Outcome`], when the instance is not valid.
    pub fn check(&self, instance: &Value) -> Result<(), Outcome> {
        self.validate(instance, Mode::All).into_result()
    }

    /// Checks a value that serializes to JSON.
    ///
    /// ```
    /// # use ash_jsonschema_builder::{Schema, Validator, object};
    /// # use serde::Serialize;
    /// #[derive(Serialize)]
    /// struct User { id: String, age: u8 }
    ///
    /// let schema = object! {
    ///     required "id": Schema::string().min_length(1),
    ///     required "age": Schema::integer().minimum(18),
    /// }.build()?;
    /// let validator = Validator::new(&schema)?;
    ///
    /// let user = User { id: "u1".into(), age: 30 };
    /// assert!(validator.check_value(&user).unwrap().is_valid());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Whatever [`serde_json::to_value`] reports, when the value will not
    /// serialize. A failing *validation* is in the [`Outcome`], not here.
    pub fn check_value<T: serde::Serialize>(
        &self,
        value: &T,
    ) -> Result<Outcome, serde_json::Error> {
        Ok(self.validate(&serde_json::to_value(value)?, Mode::All))
    }

    /// The schema this validator checks against, as a document.
    ///
    /// Borrowed, and there is no way to obtain it mutably — a validator's
    /// schema is fixed for its life.
    pub fn schema(&self) -> &Value {
        self.compiled.root()
    }

    /// An owned copy of the schema this validator checks against.
    ///
    /// The copy is detached, as [`Schema::export`] is: editing it has no effect
    /// on the validator, which keeps checking what it was built with.
    pub fn export_schema(&self) -> Option<Schema> {
        Schema::from_value(self.compiled.root().clone()).ok()
    }
}

/// Builds a [`Validator`] from a schema written inline, at the call site.
///
/// The schema is a builder chain, an [`object!`](crate::object) block, or
/// anything else that makes a [`Schema`] — and what comes back is a validator
/// whose schema is fixed. There is no way to reach in and change it afterwards;
/// to validate against something else, write another one.
///
/// ```
/// # use ash_jsonschema_builder::{Schema, validator, object};
/// let user = validator! {
///     object! {
///         required "id": Schema::string().min_length(1),
///         required "age": Schema::integer().minimum(0),
///     }
///     .closed()
/// }?;
///
/// assert!(user.is_valid(&serde_json::json!({"id": "u1", "age": 30})));
/// assert!(!user.is_valid(&serde_json::json!({"id": "u1"})));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// A registry can be given for a schema whose references reach other documents,
/// separated by `;` — the same order [`defs!`](crate::defs) uses:
///
/// ```
/// # use ash_jsonschema_builder::{Registry, Schema, validator};
/// # let address = Schema::object().id("https://example.com/address")
/// #     .required_property("city", Schema::string()).build()?;
/// # let user = Schema::object().id("https://example.com/user")
/// #     .required_property("address", Schema::reference("address")).build()?;
/// # let mut registry = Registry::new();
/// # registry.insert(&address)?;
/// # registry.insert(&user)?;
/// let validate_user = validator! { &registry; user }?;
///
/// assert!(validate_user.is_valid(&serde_json::json!({"address": {"city": "Athens"}})));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// Both forms produce a `Result<Validator, CompileError>`, since a schema whose
/// `pattern` will not compile cannot become a validator.
#[macro_export]
macro_rules! validator {
    ($registry:expr; $schema:expr $(,)?) => {
        $crate::Validator::with_registry(&$crate::Schema::from($schema), $registry)
    };
    ($schema:expr $(,)?) => {
        $crate::Validator::new(&$crate::Schema::from($schema))
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Schema, object};

    #[test]
    fn a_validator_is_cheap_to_clone_and_shares_its_schema() {
        let validator = Validator::new(&Schema::string().build().unwrap()).unwrap();
        let clone = validator.clone();

        assert!(Arc::ptr_eq(&validator.compiled, &clone.compiled));
        assert!(clone.is_valid(&serde_json::json!("x")));
    }

    #[test]
    fn a_validator_is_shareable_across_threads() {
        let schema = object! { required "id": Schema::string() }.build().unwrap();
        let validator = Validator::new(&schema).unwrap();

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let validator = validator.clone();
                std::thread::spawn(move || {
                    validator.is_valid(&serde_json::json!({"id": "x"}))
                })
            })
            .collect();

        for handle in handles {
            assert!(handle.join().unwrap());
        }
    }

    #[test]
    fn the_schema_can_be_read_but_the_copy_is_detached() {
        let schema = Schema::string().title("Fixed").build().unwrap();
        let validator = Validator::new(&schema).unwrap();

        assert_eq!(
            validator.schema().get("title"),
            Some(&serde_json::json!("Fixed")),
        );

        let mut exported = validator.export_schema().unwrap();
        exported.as_object_mut().unwrap().title = Some("Changed".into());

        // The validator still holds what it was built with.
        assert_eq!(
            validator.schema().get("title"),
            Some(&serde_json::json!("Fixed")),
        );
    }

    #[test]
    fn the_macro_matches_the_call_it_stands_for() {
        let by_macro = validator! {
            object! { required "id": Schema::string() }
        }
        .unwrap();

        let schema = object! { required "id": Schema::string() }.build().unwrap();
        let by_hand = Validator::new(&schema).unwrap();

        assert_eq!(by_macro.schema(), by_hand.schema());
    }

    #[test]
    fn check_returns_the_outcome_as_an_error() {
        let validator = Validator::new(&Schema::string().build().unwrap()).unwrap();

        assert!(validator.check(&serde_json::json!("ok")).is_ok());
        let outcome = validator.check(&serde_json::json!(1)).unwrap_err();
        assert_eq!(outcome.errors.len(), 1);
    }

    #[test]
    fn check_value_serializes_before_checking() {
        #[derive(serde::Serialize)]
        struct User {
            id: String,
        }

        let schema = object! { required "id": Schema::string().min_length(2) }
            .build()
            .unwrap();
        let validator = Validator::new(&schema).unwrap();

        assert!(
            validator
                .check_value(&User { id: "ok".into() })
                .unwrap()
                .is_valid()
        );
        assert!(
            !validator
                .check_value(&User { id: "x".into() })
                .unwrap()
                .is_valid()
        );
    }
}

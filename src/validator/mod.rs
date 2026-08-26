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
pub mod formats;
mod outcome;
pub mod tld;

use std::sync::Arc;

use serde_json::Value;

pub use compile::CompileError;
pub use formats::FormatFn;
pub use outcome::{Mode, Outcome, OutputFormat, Unit, ValidationError};

use crate::registry::Registry;
use crate::schema::Schema;
use compile::Compiled;
use formats::Formats;

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
    formats: Arc<Formats>,
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

    /// Starts a validator that needs more than the defaults — a `format` to
    /// assert, or one of your own.
    ///
    /// ```
    /// # use ash_jsonschema_builder::{Schema, Validator};
    /// let schema = Schema::string().format("employee-id").build()?;
    ///
    /// let validator = Validator::builder(&schema)
    ///     .format("employee-id", |value| value.starts_with("E-"))
    ///     .build()?;
    ///
    /// assert!(validator.is_valid(&serde_json::json!("E-1")));
    /// assert!(!validator.is_valid(&serde_json::json!("X-1")));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn builder(schema: &Schema) -> ValidatorBuilder {
        ValidatorBuilder::new(Arc::new(schema.to_value()))
    }

    /// The same, for a document that did not come from this crate's builders.
    pub fn builder_for_document(document: &Value) -> ValidatorBuilder {
        ValidatorBuilder::new(Arc::new(document.clone()))
    }

    fn build(document: Arc<Value>, registry: Option<&Registry>) -> Result<Self, CompileError> {
        let mut builder = ValidatorBuilder::new(document);
        builder.registry = registry.cloned();
        builder.build()
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
        eval::run(&self.compiled, &self.formats, instance, mode, false)
    }

    /// Checks `instance`, and collects the annotations it produced.
    ///
    /// A separate method rather than a flag on [`validate`](Validator::validate)
    /// because annotations cost a clone per annotation keyword per instance
    /// location, and the overwhelmingly common question — *is this valid?* —
    /// should not pay for an answer it will not read.
    ///
    /// Annotations are what `title`, `description`, `default`, `readOnly` and
    /// their kind contribute: they assert nothing, and instead say something
    /// about the location they were reached at. A run that fails produces
    /// none, since an annotation from a rejected schema describes an instance
    /// that does not exist.
    ///
    /// ```
    /// # use ash_jsonschema_builder::{Schema, Validator};
    /// # use ash_jsonschema_builder::validator::Mode;
    /// let schema = Schema::parse(r#"{
    ///     "properties": {
    ///         "id": {"type": "string", "title": "Identifier", "readOnly": true}
    ///     }
    /// }"#)?;
    ///
    /// let validator = Validator::new(&schema)?;
    /// let outcome = validator.annotate(&serde_json::json!({"id": "x"}), Mode::All);
    ///
    /// assert!(outcome.is_valid());
    /// let titles: Vec<_> = outcome
    ///     .annotations
    ///     .iter()
    ///     .filter(|unit| unit.keyword() == "title")
    ///     .map(|unit| unit.instance_location.as_str())
    ///     .collect();
    /// assert_eq!(titles, ["/id"]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn annotate(&self, instance: &Value, mode: Mode) -> Outcome {
        eval::run(&self.compiled, &self.formats, instance, mode, true)
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

/// Assembles a [`Validator`] that needs more than the defaults.
///
/// Obtained from [`Validator::builder`]. Every method returns the builder, so
/// the whole thing reads as one chain ending in
/// [`build`](ValidatorBuilder::build).
pub struct ValidatorBuilder {
    document: Arc<Value>,
    registry: Option<Registry>,
    formats: Formats,
}

impl ValidatorBuilder {
    fn new(document: Arc<Value>) -> Self {
        Self {
            document,
            registry: None,
            formats: Formats::default(),
        }
    }

    /// Resolves the schema's references through `registry`.
    pub fn registry(mut self, registry: &Registry) -> Self {
        self.registry = Some(registry.clone());
        self
    }

    /// Asserts every `format` this crate implements, rather than treating them
    /// as annotations.
    ///
    /// 2020-12 makes `format` an annotation by default, and this crate follows
    /// that — a schema whose `format` rejects data here and passes elsewhere is
    /// a poor contract, so turning it on is a decision the caller makes.
    ///
    /// Covers `date`, `date-time`, `time`, `duration`, `email`, `idn-email`,
    /// `hostname`, `ipv4`, `ipv6`, `uuid`, `json-pointer`, and
    /// `relative-json-pointer`. A standard name outside that set stays an
    /// annotation.
    ///
    /// ```
    /// # use ash_jsonschema_builder::{Schema, Validator};
    /// let schema = Schema::string().format("date").build()?;
    /// let validator = Validator::builder(&schema).assert_formats().build()?;
    ///
    /// assert!(validator.is_valid(&serde_json::json!("2024-02-29")));
    /// assert!(!validator.is_valid(&serde_json::json!("2023-02-29")));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn assert_formats(mut self) -> Self {
        self.formats.assert_standard();
        self
    }

    /// Asserts one `format` with a check of your own.
    ///
    /// The name may be anything — one the specification defines, in which case
    /// this replaces the built-in check, or one only your schemas use. Either
    /// way the schema stays ordinary JSON: another validator annotates the
    /// keyword where this one asserts it.
    ///
    /// A handler runs whether or not
    /// [`assert_formats`](Self::assert_formats) was called, and only ever sees
    /// string instances — `format` says nothing about any other type.
    ///
    /// ```
    /// # use ash_jsonschema_builder::{Schema, Validator};
    /// # #[cfg(feature = "iana-tlds")] {
    /// # use ash_jsonschema_builder::validator::tld::has_registered_tld;
    /// let schema = Schema::string().format("hostname").build().unwrap();
    ///
    /// let validator = Validator::builder(&schema)
    ///     .format("hostname", has_registered_tld)
    ///     .build()
    ///     .unwrap();
    ///
    /// assert!(validator.is_valid(&serde_json::json!("example.com")));
    /// assert!(!validator.is_valid(&serde_json::json!("example.invalid-tld")));
    /// # }
    /// ```
    pub fn format<F>(mut self, name: impl Into<String>, check: F) -> Self
    where
        F: Fn(&str) -> bool + Send + Sync + 'static,
    {
        self.formats.insert(name, Arc::new(check));
        self
    }

    /// Compiles the schema.
    ///
    /// # Errors
    ///
    /// As [`Validator::new`]: a `pattern` that will not compile, or a document
    /// that is not a schema.
    pub fn build(self) -> Result<Validator, CompileError> {
        Ok(Validator {
            compiled: Arc::new(Compiled::new(self.document, self.registry.as_ref())?),
            formats: Arc::new(self.formats),
        })
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

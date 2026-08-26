//! # ash-jsonschema-builder
//!
//! **Chain the keywords, call `.build()`, get a schema.**
//!
//! A typed builder for [JSON Schema 2020-12]. One dialect, deliberately: the
//! latest standard is the only thing this crate emits, so there is no draft
//! selector to thread through the API and no keyword that means one thing here
//! and another there.
//!
//! ```
//! use ash_jsonschema_builder::{Format, Schema, SchemaError};
//!
//! # fn main() -> Result<(), SchemaError> {
//! let schema = Schema::object()
//!     .dialect()
//!     .id("https://example.com/schemas/user")
//!     .title("User")
//!     .required_property("id", Schema::string().format(Format::Uuid))
//!     .required_property("email", Schema::string().format(Format::Email))
//!     .property("age", Schema::integer().minimum(0).nullable())
//!     .property("tags", Schema::array().items(Schema::string()).unique_items(true))
//!     .additional_properties(false)
//!     .build()?;
//!
//! println!("{}", schema.to_json_pretty());
//! # Ok(())
//! # }
//! ```
//!
//! ## The shape of the API
//!
//! [`Schema`] is both the output type and the entry point. Each constructor
//! returns a builder that has already committed the `type` keyword:
//!
//! | Entry point | `type` | Builder |
//! |---|---|---|
//! | [`Schema::string`] | `string` | [`StringBuilder`] |
//! | [`Schema::number`] / [`Schema::integer`] | `number` / `integer` | [`NumericBuilder`] |
//! | [`Schema::object`] | `object` | [`ObjectBuilder`] |
//! | [`Schema::array`] | `array` | [`ArrayBuilder`] |
//! | [`Schema::boolean`] / [`Schema::null`] | `boolean` / `null` | [`AnyBuilder`] |
//! | [`Schema::builder`], [`Schema::all_of`], [`Schema::reference`], … | *(none)* | [`AnyBuilder`] |
//!
//! The type is not decoration. The keywords of the core, metadata, and
//! applicator vocabularies are available on every builder, but a type-specific
//! assertion is only reachable from the builder it applies to — `.min_length()`
//! exists on a string schema and nowhere else, so a keyword that a validator
//! would silently ignore does not compile in the first place.
//!
//! ## Nesting
//!
//! Only the outermost chain needs `.build()`. Every keyword that takes a
//! subschema accepts `impl Into<Schema>`, which a builder satisfies directly —
//! and so does a bare `bool`, since `true` and `false` are themselves schemas:
//!
//! ```
//! # use ash_jsonschema_builder::Schema;
//! let schema = Schema::object()
//!     .property("nested", Schema::object().additional_properties(false))
//!     .property("anything", true)
//!     .build()
//!     .unwrap();
//! ```
//!
//! ## `.build()` checks its work
//!
//! The 2020-12 meta-schema — all eight of its documents — is vendored into the
//! crate and parsed once into memory on first use. Nothing is fetched at
//! runtime. [`build`](Builder::build) runs the document you just assembled
//! against it and returns a [`SchemaError`] rather than a `Schema` if it does
//! not hold up.
//!
//! The check is a depth-first walk, so the root and every subschema beneath it
//! are validated in turn, each in its own right — a nested builder needs no
//! `.build()` of its own and is checked all the same. A [`Violation`] names the
//! JSON Pointer of the offending subschema and the meta-schema rule it broke:
//!
//! ```
//! # use ash_jsonschema_builder::Schema;
//! let error = Schema::object()
//!     .property("inner", Schema::string().anchor("not a valid anchor"))
//!     .build()
//!     .unwrap_err();
//!
//! assert_eq!(error.first().location, "/properties/inner/$anchor");
//! assert_eq!(error.first().rule, "meta/core#/$defs/anchorString/pattern");
//! ```
//!
//! [`build_unchecked`](Builder::build_unchecked) skips the check, and
//! [`Schema::validate`] runs it on demand.
//!
//! ## Reading a schema back in
//!
//! [`Schema::parse`] reads a document that already exists, and
//! [`Schema::parse_checked`] reads and validates in one step. Keywords outside
//! the dialect are kept in [`SchemaObject::extensions`] rather than dropped, so
//! a document parsed and rendered back out is unchanged;
//! [`keyword`](Builder::keyword) writes one from the builder side.
//!
//! ```
//! # use ash_jsonschema_builder::Schema;
//! let schema = Schema::parse(r#"{"type":"object","properties":{"id":{"type":"string"}}}"#)
//!     .unwrap();
//!
//! assert!(schema.property("id").is_some());
//! assert_eq!(schema.pointer("/properties/id/type"), Some(serde_json::json!("string")));
//! ```
//!
//! [`Schema::into_builder`] picks such a schema back up and resumes the chain
//! on it; [`Schema::into_typed`] does the same under a type marker, reopening
//! that type's vocabulary.
//!
//! ## Rendering
//!
//! [`Schema`] is [`serde::Serialize`], so it drops into any serializer.
//! [`to_json`](Schema::to_json), [`to_json_pretty`](Schema::to_json_pretty),
//! and [`to_value`](Schema::to_value) are the shorthands. Output is
//! deterministic: keywords render in the specification's own grouping order,
//! and `properties`, `$defs`, and the other named-subschema maps render in the
//! order the builder calls declared them.
//!
//! ## `$ref` resolves against a [`Registry`]
//!
//! Following a reference needs a base URI to resolve against and somewhere to
//! look the result up. A [`Registry`] is that somewhere: schemas go in under
//! the `$id` they declare, and references resolve across documents, not only
//! within one.
//!
//! ```
//! # use ash_jsonschema_builder::{Registry, Schema};
//! let address = Schema::object()
//!     .id("https://example.com/schemas/address")
//!     .required_property("city", Schema::string())
//!     .build()?;
//!
//! let user = Schema::object()
//!     .id("https://example.com/schemas/user")
//!     .required_property("address", Schema::reference("address"))
//!     .build()?;
//!
//! let mut registry = Registry::new();
//! registry.insert(&address)?;
//! registry.insert(&user)?;
//!
//! registry.check_references()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Nothing is fetched: a registry resolves against what it was given, so a
//! reference to a document nobody inserted dangles rather than reaching the
//! network.
//!
//! ## A schema is a source, not just an output
//!
//! [`diff()`] compares two versions and says what the change breaks — separately
//! for whoever writes the data and whoever reads it, since a change lands on
//! those two in opposite directions:
//!
//! ```
//! # use ash_jsonschema_builder::{Schema, diff::{diff, Role}};
//! let v1 = Schema::object().required_property("id", Schema::string()).build()?;
//! let v2 = Schema::object()
//!     .required_property("id", Schema::string())
//!     .required_property("email", Schema::string())
//!     .build()?;
//!
//! let report = diff(&v1, &v2);
//! assert!(report.is_breaking_for(Role::Writer));
//! assert!(!report.is_breaking_for(Role::Reader));
//! # Ok::<(), ash_jsonschema_builder::SchemaError>(())
//! ```
//!
//! [`bundle()`] collapses a set of schemas into one self-contained document,
//! embedding each referenced document under `$defs` with its `$id` intact so
//! the references that named it still resolve.
//!
//! ## Macros
//!
//! [`object!`] writes `properties` and `required` from one list — the
//! duplication object schemas otherwise force — and yields an ordinary builder:
//!
//! ```
//! # use ash_jsonschema_builder::{Schema, object};
//! let user = object! {
//!     required "id": Schema::string().format("uuid"),
//!     "age": Schema::integer().minimum(0),
//! }
//! .closed()
//! .build()?;
//!
//! assert!(user.requires("id"));
//! # Ok::<(), ash_jsonschema_builder::SchemaError>(())
//! ```
//!
//! [`defs!`] takes a block of definitions; [`one_of_tagged!`] writes the tagged
//! union the dialect has no keyword for. They exist for shapes that repeat
//! *structure*, not to shorten single keywords.
//!
//! ## Getting a copy out
//!
//! [`Schema::export`] returns a detached copy, and
//! [`Schema::export_at`] lifts a subschema out as a [`Schema`] rather than a
//! bare [`serde_json::Value`]. Both are on [`FrozenSchema`] too, and `export`
//! is on every builder — where it takes a copy without ending the chain.
//!
//! ## Validating instances
//!
//! [`Validator`] answers the other question: does this JSON satisfy the schema?
//!
//! ```
//! # use ash_jsonschema_builder::{Schema, Validator, object};
//! # use ash_jsonschema_builder::validator::Mode;
//! let validator = Validator::new(&object! {
//!     required "id": Schema::string().min_length(1),
//! }.build()?)?;
//!
//! assert!(validator.is_valid(&serde_json::json!({"id": "a1"})));
//!
//! let outcome = validator.validate(&serde_json::json!({}), Mode::All);
//! assert_eq!(outcome.first().unwrap().message, "`id` is required");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! A validator's schema is fixed for its life. [`Mode::All`] reports every
//! error and [`Mode::FailFast`] stops at the first; both give an [`Outcome`]
//! whose errors name where in the instance the problem is *and* which schema
//! keyword rejected it.
//!
//! Coverage is every assertion and applicator of the dialect, including
//! `unevaluated*` and `$dynamicRef`. `format` and the content vocabulary are
//! annotations in 2020-12 and are not asserted.
//!
//! ## What this crate does not do
//!
//! It **builds**, **reads**, and **checks against** schemas. What it does not
//! do is fetch — no `$ref` is retrieved over the network. The
//! meta-schema check `.build()` runs constrains a schema's *shape* — that
//! `$anchor` is a plain name, that `required` holds unique strings, that
//! `minLength` is not negative — and says nothing about whether the schema
//! makes sense. A `minLength` above its `maxLength`, or a `then` without an
//! `if`, is a legal 2020-12 document and builds without complaint.
//!
//! References are followed only when asked: `.build()` does not resolve them,
//! and a [`Registry`] does.
//!
//! [JSON Schema 2020-12]: https://json-schema.org/draft/2020-12/release-notes

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod builder;
pub mod bundle;
pub mod diff;
pub mod error;
pub mod format;
pub mod frozen;
pub mod macros;
pub mod map;
pub mod meta;
pub mod registry;
pub mod schema;
pub mod uri;
pub mod validator;
mod validate;
pub mod walk;

pub use builder::{
    AnyBuilder, ArrayBuilder, Builder, NumericBuilder, ObjectBuilder, StringBuilder, kind,
};
pub use bundle::{BundleError, bundle};
pub use diff::{Change, ChangeKind, Effect, Report, Role, diff, diff_resolved};
pub use error::{ErrorKind, ParseError, SchemaError, Violation};
pub use format::Format;
pub use frozen::FrozenSchema;
pub use map::OrderedMap;
pub use meta::MetaSchema;
pub use registry::{Reference, Registry, RegistryError};
pub use schema::{
    DIALECT, DIALECT_KEYWORDS, Num, Schema, SchemaObject, SchemaType, TypeSet, UnknownSchemaType,
    is_dialect_keyword,
};
pub use validate::validate_document;
pub use validator::{CompileError, Mode, Outcome, ValidationError, Validator};

/// The types a typical caller wants in scope.
///
/// ```
/// use ash_jsonschema_builder::prelude::*;
///
/// let schema = Schema::object().property("name", Schema::string()).build().unwrap();
/// ```
pub mod prelude {
    pub use crate::{
        Format, Num, ParseError, Schema, SchemaError, SchemaObject, SchemaType, TypeSet,
        Violation,
    };
}

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
//! ## Rendering
//!
//! [`Schema`] is [`serde::Serialize`], so it drops into any serializer.
//! [`to_json`](Schema::to_json), [`to_json_pretty`](Schema::to_json_pretty),
//! and [`to_value`](Schema::to_value) are the shorthands. Output is
//! deterministic: keywords render in the specification's own grouping order,
//! and `properties`, `$defs`, and the other named-subschema maps render in the
//! order the builder calls declared them.
//!
//! ## What this crate does not do
//!
//! It **builds** schemas; it does not validate instances against them. The
//! meta-schema check `.build()` runs constrains a schema's *shape* — that
//! `$anchor` is a plain name, that `required` holds unique strings, that
//! `minLength` is not negative — and says nothing about whether the schema
//! makes sense. A `minLength` above its `maxLength`, or a `then` without an
//! `if`, is a legal 2020-12 document and builds without complaint.
//!
//! [JSON Schema 2020-12]: https://json-schema.org/draft/2020-12/release-notes

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod builder;
pub mod error;
pub mod format;
pub mod frozen;
pub mod map;
pub mod meta;
pub mod schema;
mod validate;

pub use builder::{
    AnyBuilder, ArrayBuilder, Builder, NumericBuilder, ObjectBuilder, StringBuilder, kind,
};
pub use error::{SchemaError, Violation};
pub use format::Format;
pub use frozen::FrozenSchema;
pub use map::OrderedMap;
pub use meta::MetaSchema;
pub use schema::{DIALECT, Num, Schema, SchemaObject, SchemaType, TypeSet};
pub use validate::validate_document;

/// The types a typical caller wants in scope.
///
/// ```
/// use ash_jsonschema_builder::prelude::*;
///
/// let schema = Schema::object().property("name", Schema::string()).build().unwrap();
/// ```
pub mod prelude {
    pub use crate::{
        Format, Num, Schema, SchemaError, SchemaObject, SchemaType, TypeSet, Violation,
    };
}

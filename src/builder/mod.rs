//! The builders.
//!
//! Every builder is the same type — [`Builder<K>`] — carrying a zero-sized
//! marker `K` that records which instance type it is building for. The marker
//! is what makes `.min_length(1)` reachable on a string schema and unreachable
//! on a number one: the keywords of the general vocabularies live on
//! `impl<K> Builder<K>`, and each type-specific vocabulary lives on an impl for
//! its own marker.
//!
//! Builders are consuming — every method takes `self` and returns it — so a
//! chain reads as one expression and `.build()` ends it.
//!
//! ```
//! # use ash_jsonschema_builder::Schema;
//! let schema = Schema::array()
//!     .description("A non-empty set of tags")
//!     .items(Schema::string().min_length(1))
//!     .min_items(1)
//!     .unique_items(true)
//!     .build().unwrap();
//! ```
//!
//! Nested schemas do **not** need their own `.build()`: every keyword that
//! takes a subschema accepts `impl Into<Schema>`, which a builder, a
//! [`Schema`], and a bare `bool` all satisfy.

mod array;
mod numeric;
mod object;
mod string;

use std::marker::PhantomData;

use serde_json::Value;

use crate::error::SchemaError;
use crate::frozen::FrozenSchema;
use crate::map::OrderedMap;
use crate::schema::{DIALECT, Schema, SchemaObject, SchemaType, TypeSet};

/// The zero-sized markers that tell one builder flavour from another.
pub mod kind {
    /// No committed `type` — combinators, `$ref`s, `const`/`enum` schemas.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct Any;
    /// `"type": "string"`.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct Str;
    /// `"type": "number"` or `"type": "integer"`.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct Numeric;
    /// `"type": "object"`.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct Object;
    /// `"type": "array"`.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct Array;
}

/// A builder for schemas with no committed instance type.
pub type AnyBuilder = Builder<kind::Any>;
/// A builder for string schemas.
pub type StringBuilder = Builder<kind::Str>;
/// A builder for number and integer schemas.
pub type NumericBuilder = Builder<kind::Numeric>;
/// A builder for object schemas.
pub type ObjectBuilder = Builder<kind::Object>;
/// A builder for array schemas.
pub type ArrayBuilder = Builder<kind::Array>;

/// Accumulates keywords, then hands back a [`Schema`] on [`build`](Builder::build).
///
/// Obtain one from a [`Schema`] entry point — [`Schema::string`],
/// [`Schema::object`], [`Schema::builder`], and friends — rather than
/// constructing it directly; those are what set the `type` keyword that `K`
/// stands for.
#[derive(Clone, Debug, PartialEq)]
pub struct Builder<K> {
    pub(crate) inner: SchemaObject,
    kind: PhantomData<K>,
}

impl<K> Builder<K> {
    /// A builder with no keywords set.
    pub(crate) fn new() -> Self {
        Self {
            inner: SchemaObject::new(),
            kind: PhantomData,
        }
    }

    /// A builder with `type` already committed to `ty`.
    pub(crate) fn of_type(ty: SchemaType) -> Self {
        let mut builder = Self::new();
        builder.inner.types = Some(TypeSet::One(ty));
        builder
    }

    /// Finishes the chain, checks the result against the embedded JSON Schema
    /// 2020-12 meta-schema, and produces the schema.
    ///
    /// The check walks the whole document — the root and every subschema under
    /// it, including the ones nested builders contributed — so nesting needs no
    /// `.build()` of its own and gets validated all the same. A failure names
    /// the JSON Pointer of the offending subschema and the meta-schema rule it
    /// broke; see [`SchemaError`].
    ///
    /// ```
    /// # use ash_jsonschema_builder::Schema;
    /// // `$anchor` must be a plain name, and the meta-schema says so.
    /// let error = Schema::object()
    ///     .property("inner", Schema::string().anchor("not a valid anchor"))
    ///     .build()
    ///     .unwrap_err();
    ///
    /// assert_eq!(error.first().location, "/properties/inner/$anchor");
    /// ```
    ///
    /// The meta-schema constrains a schema's *shape*, not its sense: it rejects
    /// a malformed `$anchor` or a negative `minLength`, and has nothing to say
    /// about a `minLength` above its `maxLength`. Use
    /// [`build_unchecked`](Self::build_unchecked) to skip the check entirely.
    ///
    /// Documents nested past roughly fifty levels of subschema report a depth
    /// violation instead of being checked to the bottom — the evaluator stops
    /// rather than risk a stack overflow, which would abort the process
    /// uncatchably. Such a document still builds through
    /// [`build_unchecked`](Self::build_unchecked), though rendering a tree that
    /// deep recurses just as hard.
    pub fn build(self) -> Result<Schema, SchemaError> {
        let schema = self.build_unchecked();
        schema.validate()?;
        Ok(schema)
    }

    /// Finishes the chain, checks the result, and freezes it so it can never be
    /// modified again.
    ///
    /// The same check [`build`](Self::build) runs, wrapped in a
    /// [`FrozenSchema`] — a cheaply cloned, `Send + Sync` handle that hands out
    /// `&Schema` and nothing more. This is the form to reach for once a schema
    /// is finished: built once at startup, shared to as many readers as you
    /// like, and immune to being edited out from under any of them.
    ///
    /// ```
    /// # use ash_jsonschema_builder::Schema;
    /// let schema = Schema::object()
    ///     .required_property("id", Schema::string().format("uuid"))
    ///     .build_frozen()?;
    ///
    /// // Reads work as they do on a `Schema`; there is no way to write.
    /// assert_eq!(schema.as_object().unwrap().required.as_ref().unwrap(), &["id"]);
    ///
    /// // Cloning shares the one schema rather than copying it.
    /// let shared = schema.clone();
    /// assert!(schema.is_same_as(&shared));
    /// # Ok::<(), ash_jsonschema_builder::SchemaError>(())
    /// ```
    ///
    /// [`FrozenSchema::thaw`] clones a mutable copy back out, leaving the
    /// frozen one as it was.
    pub fn build_frozen(self) -> Result<FrozenSchema, SchemaError> {
        self.build().map(FrozenSchema::new)
    }

    /// Finishes the chain without checking the result against the meta-schema.
    ///
    /// This is the conversion a nested builder goes through, since a subschema
    /// is validated in place by whichever [`build`](Self::build) call finishes
    /// the document it lands in.
    pub fn build_unchecked(self) -> Schema {
        Schema::Object(Box::new(self.inner))
    }

    /// Finishes the chain and produces the bare keyword bag, for callers who
    /// want to keep editing it by hand.
    pub fn into_object(self) -> SchemaObject {
        self.inner
    }

    // --- core vocabulary --------------------------------------------------

    /// Sets `$schema` to the 2020-12 dialect URI. Call this on the root schema
    /// of a document; subschemas inherit the dialect and must not repeat it.
    pub fn dialect(self) -> Self {
        self.schema_uri(DIALECT)
    }

    /// Sets `$schema` to an arbitrary dialect URI. Prefer [`dialect`](Self::dialect).
    pub fn schema_uri(mut self, uri: impl Into<String>) -> Self {
        self.inner.schema = Some(uri.into());
        self
    }

    /// Sets `$id`, the base URI this schema and its children resolve against.
    pub fn id(mut self, uri: impl Into<String>) -> Self {
        self.inner.id = Some(uri.into());
        self
    }

    /// Sets `$anchor`, a plain-name fragment target within the base URI.
    pub fn anchor(mut self, name: impl Into<String>) -> Self {
        self.inner.anchor = Some(name.into());
        self
    }

    /// Sets `$dynamicAnchor`.
    pub fn dynamic_anchor(mut self, name: impl Into<String>) -> Self {
        self.inner.dynamic_anchor = Some(name.into());
        self
    }

    /// Sets `$ref`.
    ///
    /// In 2020-12 `$ref` sits alongside other keywords rather than replacing
    /// them, so a schema may both reference and constrain.
    pub fn reference(mut self, uri: impl Into<String>) -> Self {
        self.inner.reference = Some(uri.into());
        self
    }

    /// Sets `$dynamicRef`.
    pub fn dynamic_ref(mut self, uri: impl Into<String>) -> Self {
        self.inner.dynamic_ref = Some(uri.into());
        self
    }

    /// Sets `$comment` — a note for readers of the schema, which validators
    /// ignore.
    pub fn comment(mut self, text: impl Into<String>) -> Self {
        self.inner.comment = Some(text.into());
        self
    }

    /// Declares one `$vocabulary` entry: its URI, and whether an implementation
    /// that does not understand it must refuse the schema.
    pub fn vocabulary(mut self, uri: impl Into<String>, required: bool) -> Self {
        self.inner
            .vocabulary
            .get_or_insert_with(OrderedMap::new)
            .insert(uri, required);
        self
    }

    /// Adds one named subschema to `$defs`.
    pub fn def(mut self, name: impl Into<String>, schema: impl Into<Schema>) -> Self {
        self.inner
            .defs
            .get_or_insert_with(OrderedMap::new)
            .insert(name, schema.into());
        self
    }

    /// Adds several named subschemas to `$defs`.
    pub fn defs<N, S>(mut self, defs: impl IntoIterator<Item = (N, S)>) -> Self
    where
        N: Into<String>,
        S: Into<Schema>,
    {
        let target = self.inner.defs.get_or_insert_with(OrderedMap::new);
        for (name, schema) in defs {
            target.insert(name, schema.into());
        }
        self
    }

    // --- metadata vocabulary ----------------------------------------------

    /// Sets `title`.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.inner.title = Some(title.into());
        self
    }

    /// Sets `description`.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.inner.description = Some(description.into());
        self
    }

    /// Sets `default`. This is a suggestion for consumers; nothing asserts that
    /// the value validates against the schema it sits in.
    pub fn default(mut self, value: impl Into<Value>) -> Self {
        self.inner.default = Some(value.into());
        self
    }

    /// Sets `deprecated`.
    pub fn deprecated(mut self, deprecated: bool) -> Self {
        self.inner.deprecated = Some(deprecated);
        self
    }

    /// Sets `readOnly`.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.inner.read_only = Some(read_only);
        self
    }

    /// Sets `writeOnly`.
    pub fn write_only(mut self, write_only: bool) -> Self {
        self.inner.write_only = Some(write_only);
        self
    }

    /// Appends one value to `examples`.
    pub fn example(mut self, value: impl Into<Value>) -> Self {
        self.inner
            .examples
            .get_or_insert_with(Vec::new)
            .push(value.into());
        self
    }

    /// Appends several values to `examples`.
    pub fn examples(mut self, values: impl IntoIterator<Item = impl Into<Value>>) -> Self {
        let target = self.inner.examples.get_or_insert_with(Vec::new);
        target.extend(values.into_iter().map(Into::into));
        self
    }

    // --- validation: any instance -----------------------------------------

    /// Sets `const` — the instance must equal this value.
    pub fn constant(mut self, value: impl Into<Value>) -> Self {
        self.inner.constant = Some(value.into());
        self
    }

    /// Sets `enum` — the instance must equal one of these values.
    ///
    /// ```
    /// # use ash_jsonschema_builder::Schema;
    /// let schema = Schema::string().enumeration(["red", "green", "blue"]).build().unwrap();
    /// assert_eq!(schema.to_json(), r#"{"type":"string","enum":["red","green","blue"]}"#);
    /// ```
    pub fn enumeration(mut self, values: impl IntoIterator<Item = impl Into<Value>>) -> Self {
        self.inner.enumeration = Some(values.into_iter().map(Into::into).collect());
        self
    }

    /// Adds `null` to the permitted `type`s.
    ///
    /// This widens the `type` keyword rather than wrapping the schema in an
    /// `anyOf`, so the sibling assertions keep applying to the non-null case.
    ///
    /// ```
    /// # use ash_jsonschema_builder::Schema;
    /// let schema = Schema::string().min_length(1).nullable().build().unwrap();
    /// assert_eq!(schema.to_json(), r#"{"type":["string","null"],"minLength":1}"#);
    /// ```
    pub fn nullable(mut self) -> Self {
        match &mut self.inner.types {
            Some(types) => types.insert(SchemaType::Null),
            slot @ None => *slot = Some(TypeSet::One(SchemaType::Null)),
        }
        self
    }

    /// Replaces `type` with the given set. An empty set clears the keyword; a
    /// one-element set renders bare rather than as an array.
    pub fn types(mut self, types: impl IntoIterator<Item = SchemaType>) -> Self {
        let types: TypeSet = types.into_iter().collect();
        self.inner.types = match &types {
            TypeSet::Many(many) if many.is_empty() => None,
            _ => Some(types),
        };
        self
    }

    // --- applicators -------------------------------------------------------

    /// Appends to `allOf` — every subschema must match.
    pub fn all_of(mut self, schemas: impl IntoIterator<Item = impl Into<Schema>>) -> Self {
        extend(&mut self.inner.all_of, schemas);
        self
    }

    /// Appends to `anyOf` — at least one subschema must match.
    pub fn any_of(mut self, schemas: impl IntoIterator<Item = impl Into<Schema>>) -> Self {
        extend(&mut self.inner.any_of, schemas);
        self
    }

    /// Appends to `oneOf` — exactly one subschema must match.
    pub fn one_of(mut self, schemas: impl IntoIterator<Item = impl Into<Schema>>) -> Self {
        extend(&mut self.inner.one_of, schemas);
        self
    }

    /// Sets `not` — the subschema must not match.
    pub fn not(mut self, schema: impl Into<Schema>) -> Self {
        self.inner.not = Some(Box::new(schema.into()));
        self
    }

    /// Sets `if`. On its own it asserts nothing — pair it with
    /// [`then_schema`](Self::then_schema) or [`else_schema`](Self::else_schema).
    pub fn condition(mut self, schema: impl Into<Schema>) -> Self {
        self.inner.if_schema = Some(Box::new(schema.into()));
        self
    }

    /// Sets `then`, applied when `if` matches. Ignored without an `if`.
    pub fn then_schema(mut self, schema: impl Into<Schema>) -> Self {
        self.inner.then_schema = Some(Box::new(schema.into()));
        self
    }

    /// Sets `else`, applied when `if` does not match. Ignored without an `if`.
    pub fn else_schema(mut self, schema: impl Into<Schema>) -> Self {
        self.inner.else_schema = Some(Box::new(schema.into()));
        self
    }

    /// Sets `if` and `then` together.
    pub fn if_then(self, condition: impl Into<Schema>, then: impl Into<Schema>) -> Self {
        self.condition(condition).then_schema(then)
    }

    /// Sets `if`, `then`, and `else` together.
    ///
    /// ```
    /// # use ash_jsonschema_builder::Schema;
    /// let schema = Schema::object()
    ///     .property("kind", Schema::string())
    ///     .if_then_else(
    ///         Schema::object().property("kind", Schema::builder().constant("circle")),
    ///         Schema::object().required(["radius"]),
    ///         Schema::object().required(["width", "height"]),
    ///     )
    ///     .build().unwrap();
    /// # assert!(schema.to_json().contains(r#""else""#));
    /// ```
    pub fn if_then_else(
        self,
        condition: impl Into<Schema>,
        then: impl Into<Schema>,
        otherwise: impl Into<Schema>,
    ) -> Self {
        self.condition(condition)
            .then_schema(then)
            .else_schema(otherwise)
    }
}

impl<K> From<Builder<K>> for Schema {
    /// Unchecked, deliberately: the root `.build()` validates the whole tree,
    /// so a subschema is checked exactly once, in place.
    fn from(value: Builder<K>) -> Self {
        value.build_unchecked()
    }
}

/// The keywords the specification defines generally but that are only
/// meaningful for one instance type live on that type's builder. An untyped
/// builder may legitimately need any of them — a `$ref`-and-refine schema, or
/// one whose type is pinned by a sibling `allOf` — so they are repeated here.
impl Builder<kind::Any> {
    /// Sets `format`.
    pub fn format(mut self, format: impl Into<crate::format::Format>) -> Self {
        self.inner.format = Some(format.into());
        self
    }

    /// Sets `unevaluatedProperties`.
    pub fn unevaluated_properties(mut self, schema: impl Into<Schema>) -> Self {
        self.inner.unevaluated_properties = Some(Box::new(schema.into()));
        self
    }

    /// Sets `unevaluatedItems`.
    pub fn unevaluated_items(mut self, schema: impl Into<Schema>) -> Self {
        self.inner.unevaluated_items = Some(Box::new(schema.into()));
        self
    }
}

/// Appends `schemas` to an optional list, creating it if absent.
fn extend(slot: &mut Option<Vec<Schema>>, schemas: impl IntoIterator<Item = impl Into<Schema>>) {
    let target = slot.get_or_insert_with(Vec::new);
    target.extend(schemas.into_iter().map(Into::into));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_and_into_object_agree() {
        let keywords = Schema::string().title("Name").into_object();
        assert_eq!(
            Schema::string().title("Name").build().unwrap(),
            Schema::from(keywords)
        );
    }

    #[test]
    fn a_builder_converts_into_a_schema_without_an_explicit_build() {
        let nested: Schema = Schema::integer().minimum(0).into();
        assert_eq!(nested, Schema::integer().minimum(0).build().unwrap());
    }

    #[test]
    fn dialect_sets_the_2020_12_uri() {
        let schema = Schema::builder().dialect().build().unwrap();
        assert_eq!(
            schema.as_object().unwrap().schema.as_deref(),
            Some("https://json-schema.org/draft/2020-12/schema"),
        );
    }

    #[test]
    fn nullable_widens_an_existing_type_and_seeds_an_absent_one() {
        let widened = Schema::string().nullable().build().unwrap();
        assert_eq!(widened.to_json(), r#"{"type":["string","null"]}"#);

        let seeded = Schema::builder().nullable().build().unwrap();
        assert_eq!(seeded.to_json(), r#"{"type":"null"}"#);

        let idempotent = Schema::string().nullable().nullable().build().unwrap();
        assert_eq!(idempotent, widened);
    }

    #[test]
    fn types_replaces_and_an_empty_set_clears() {
        let replaced = Schema::string()
            .types([SchemaType::Integer, SchemaType::Boolean])
            .build()
            .unwrap();
        assert_eq!(replaced.to_json(), r#"{"type":["integer","boolean"]}"#);

        let cleared = Schema::string().types([]).build().unwrap();
        assert_eq!(cleared.to_json(), "{}");
    }

    #[test]
    fn combinator_lists_append_rather_than_replace() {
        let schema = Schema::builder()
            .any_of([Schema::string()])
            .any_of([Schema::integer()])
            .build()
            .unwrap();
        assert_eq!(
            schema.as_object().unwrap().any_of.as_ref().unwrap().len(),
            2
        );
    }

    #[test]
    fn defs_and_vocabulary_accumulate_in_order() {
        let schema = Schema::builder()
            .def("A", Schema::string())
            .defs([
                ("B", Schema::integer().build().unwrap()),
                ("C", Schema::boolean().build().unwrap()),
            ])
            .vocabulary("https://json-schema.org/draft/2020-12/vocab/core", true)
            .vocabulary("https://example.com/vocab/custom", false)
            .build()
            .unwrap();

        let object = schema.as_object().unwrap();
        assert_eq!(
            object.defs.as_ref().unwrap().keys().collect::<Vec<_>>(),
            ["A", "B", "C"]
        );
        assert_eq!(object.vocabulary.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn examples_append_one_at_a_time_or_in_bulk() {
        let schema = Schema::string()
            .example("a")
            .examples(["b", "c"])
            .build()
            .unwrap();
        assert_eq!(
            schema.to_json(),
            r#"{"type":"string","examples":["a","b","c"]}"#
        );
    }

    #[test]
    fn a_bare_bool_is_a_schema() {
        assert_eq!(
            Schema::builder().not(false).build().unwrap().to_json(),
            r#"{"not":false}"#
        );
    }

    #[test]
    fn metadata_keywords_render_in_document_order() {
        let schema = Schema::builder()
            .description("desc")
            .title("title")
            .deprecated(true)
            .read_only(true)
            .write_only(false)
            .default(0)
            .build()
            .unwrap();

        assert_eq!(
            schema.to_json(),
            r#"{"title":"title","description":"desc","default":0,"deprecated":true,"readOnly":true,"writeOnly":false}"#,
        );
    }
}

//! The schema document itself: [`Schema`], the keyword bag [`SchemaObject`] it
//! wraps, and the small value types the keywords are written in.
//!
//! This module is the *data*. Nothing here enforces a construction order or a
//! keyword's meaning — that is the [`builder`](crate::builder) module's job.
//! Everything is public so a caller who needs a keyword combination the
//! builders do not expose can assemble a [`SchemaObject`] by hand.

use serde::{Serialize, Serializer};
use serde_json::Value;

use crate::builder::{
    AnyBuilder, ArrayBuilder, Builder, NumericBuilder, ObjectBuilder, StringBuilder,
};
use crate::format::Format;
use crate::map::OrderedMap;

/// The `$schema` URI of the dialect this crate builds: JSON Schema 2020-12.
pub const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

// ---------------------------------------------------------------------------
// Numbers
// ---------------------------------------------------------------------------

/// A JSON number, as written by the numeric keywords (`minimum`, `multipleOf`,
/// …).
///
/// Integers stay integers through serialization — `.minimum(0)` renders as `0`,
/// not `0.0` — because the underlying [`serde_json::Number`] remembers which it
/// was built from.
///
/// ```
/// # use ash_jsonschema_builder::Num;
/// assert_eq!(serde_json::to_string(&Num::from(3)).unwrap(), "3");
/// assert_eq!(serde_json::to_string(&Num::from(2.5)).unwrap(), "2.5");
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Num(serde_json::Number);

impl Num {
    /// Wraps a float, or returns `None` if it is NaN or infinite — JSON has no
    /// way to write either.
    pub fn try_from_f64(value: f64) -> Option<Self> {
        serde_json::Number::from_f64(value).map(Self)
    }

    /// The underlying JSON number.
    pub fn as_json_number(&self) -> &serde_json::Number {
        &self.0
    }
}

macro_rules! num_from_integer {
    ($($ty:ty),* $(,)?) => {
        $(
            impl From<$ty> for Num {
                fn from(value: $ty) -> Self {
                    Self(serde_json::Number::from(value))
                }
            }
        )*
    };
}

num_from_integer!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

impl From<f64> for Num {
    /// # Panics
    ///
    /// If `value` is NaN or infinite. JSON cannot represent either, so a
    /// non-finite bound is a bug in the caller rather than a schema this crate
    /// could emit. Use [`Num::try_from_f64`] to handle the case instead.
    fn from(value: f64) -> Self {
        Self::try_from_f64(value)
            .unwrap_or_else(|| panic!("JSON has no representation for the number {value}"))
    }
}

impl From<f32> for Num {
    /// # Panics
    ///
    /// If `value` is NaN or infinite. See [`From<f64>`](Num::from).
    fn from(value: f32) -> Self {
        Self::from(f64::from(value))
    }
}

impl From<Num> for Value {
    fn from(value: Num) -> Self {
        Value::Number(value.0)
    }
}

impl Serialize for Num {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// One of the seven values the `type` keyword accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SchemaType {
    /// `null`.
    Null,
    /// `boolean`.
    Boolean,
    /// `object`.
    Object,
    /// `array`.
    Array,
    /// `number` — any JSON number.
    Number,
    /// `integer` — a number with a zero fractional part.
    Integer,
    /// `string`.
    String,
}

impl SchemaType {
    /// The type as it is written in the schema.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Boolean => "boolean",
            Self::Object => "object",
            Self::Array => "array",
            Self::Number => "number",
            Self::Integer => "integer",
            Self::String => "string",
        }
    }
}

impl std::fmt::Display for SchemaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The value of the `type` keyword: one type, or a set of permitted ones.
///
/// The two render differently — `"type": "string"` versus
/// `"type": ["string", "null"]` — and both are valid, so the distinction is
/// kept rather than always emitting an array.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum TypeSet {
    /// Exactly one permitted type.
    One(SchemaType),
    /// A set of permitted types.
    Many(Vec<SchemaType>),
}

impl TypeSet {
    /// Whether `ty` is permitted by this set.
    pub fn contains(&self, ty: SchemaType) -> bool {
        match self {
            Self::One(one) => *one == ty,
            Self::Many(many) => many.contains(&ty),
        }
    }

    /// Adds `ty` to the set, widening a single type into a set. Adding a type
    /// that is already permitted changes nothing.
    pub fn insert(&mut self, ty: SchemaType) {
        if self.contains(ty) {
            return;
        }
        match self {
            Self::One(one) => *self = Self::Many(vec![*one, ty]),
            Self::Many(many) => many.push(ty),
        }
    }
}

impl From<SchemaType> for TypeSet {
    fn from(value: SchemaType) -> Self {
        Self::One(value)
    }
}

impl FromIterator<SchemaType> for TypeSet {
    fn from_iter<I: IntoIterator<Item = SchemaType>>(iter: I) -> Self {
        let mut types: Vec<SchemaType> = Vec::new();
        for ty in iter {
            if !types.contains(&ty) {
                types.push(ty);
            }
        }
        match <[SchemaType; 1]>::try_from(types.clone()) {
            Ok([one]) => Self::One(one),
            Err(_) => Self::Many(types),
        }
    }
}

// ---------------------------------------------------------------------------
// The keyword bag
// ---------------------------------------------------------------------------

/// Every keyword of the 2020-12 dialect, each one unset by default.
///
/// A keyword that is `None` is absent from the rendered document — JSON Schema
/// gives an absent keyword and a keyword set to its default meaning very
/// different treatment under `unevaluatedProperties`, so the two are never
/// conflated here.
///
/// Field order is output order, chosen to read the way a hand-written schema
/// does: identity, then labels, then `type` and the assertions it governs, then
/// the applicators, then the annotations, with `$defs` last.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SchemaObject {
    // --- core vocabulary -------------------------------------------------
    /// `$schema` — the dialect this document is written in.
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// `$id` — the base URI this schema and its children resolve against.
    #[serde(rename = "$id", skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// `$anchor` — a plain-name fragment target within the base URI.
    #[serde(rename = "$anchor", skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// `$dynamicAnchor` — an anchor that participates in dynamic resolution.
    #[serde(rename = "$dynamicAnchor", skip_serializing_if = "Option::is_none")]
    pub dynamic_anchor: Option<String>,
    /// `$ref` — a reference to another schema.
    #[serde(rename = "$ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// `$dynamicRef` — a reference resolved against the dynamic scope.
    #[serde(rename = "$dynamicRef", skip_serializing_if = "Option::is_none")]
    pub dynamic_ref: Option<String>,
    /// `$vocabulary` — which vocabularies a meta-schema requires.
    #[serde(rename = "$vocabulary", skip_serializing_if = "Option::is_none")]
    pub vocabulary: Option<OrderedMap<bool>>,
    /// `$comment` — a note for readers, never for validators.
    #[serde(rename = "$comment", skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,

    // --- metadata: labels -------------------------------------------------
    /// `title` — a short label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// `description` — a longer explanation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    // --- validation: any instance ----------------------------------------
    /// `type` — the permitted instance type(s).
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub types: Option<TypeSet>,
    /// `enum` — the instance must equal one of these values.
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enumeration: Option<Vec<Value>>,
    /// `const` — the instance must equal this value.
    #[serde(rename = "const", skip_serializing_if = "Option::is_none")]
    pub constant: Option<Value>,

    // --- validation: numbers ---------------------------------------------
    /// `multipleOf` — the instance divided by this must be an integer.
    #[serde(rename = "multipleOf", skip_serializing_if = "Option::is_none")]
    pub multiple_of: Option<Num>,
    /// `maximum` — inclusive upper bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<Num>,
    /// `exclusiveMaximum` — exclusive upper bound.
    #[serde(rename = "exclusiveMaximum", skip_serializing_if = "Option::is_none")]
    pub exclusive_maximum: Option<Num>,
    /// `minimum` — inclusive lower bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<Num>,
    /// `exclusiveMinimum` — exclusive lower bound.
    #[serde(rename = "exclusiveMinimum", skip_serializing_if = "Option::is_none")]
    pub exclusive_minimum: Option<Num>,

    // --- validation: strings ---------------------------------------------
    /// `maxLength` — in Unicode code points, not bytes.
    #[serde(rename = "maxLength", skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u64>,
    /// `minLength` — in Unicode code points, not bytes.
    #[serde(rename = "minLength", skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u64>,
    /// `pattern` — an ECMA-262 regular expression, unanchored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// `format` — a defined format attribute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<Format>,

    // --- validation: arrays ----------------------------------------------
    /// `prefixItems` — positional schemas for the leading elements.
    #[serde(rename = "prefixItems", skip_serializing_if = "Option::is_none")]
    pub prefix_items: Option<Vec<Schema>>,
    /// `items` — applies to every element `prefixItems` did not cover.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<Schema>>,
    /// `contains` — at least one element must match.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contains: Option<Box<Schema>>,
    /// `maxContains` — upper bound on `contains` matches.
    #[serde(rename = "maxContains", skip_serializing_if = "Option::is_none")]
    pub max_contains: Option<u64>,
    /// `minContains` — lower bound on `contains` matches.
    #[serde(rename = "minContains", skip_serializing_if = "Option::is_none")]
    pub min_contains: Option<u64>,
    /// `maxItems` — upper bound on element count.
    #[serde(rename = "maxItems", skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u64>,
    /// `minItems` — lower bound on element count.
    #[serde(rename = "minItems", skip_serializing_if = "Option::is_none")]
    pub min_items: Option<u64>,
    /// `uniqueItems` — no two elements may be equal.
    #[serde(rename = "uniqueItems", skip_serializing_if = "Option::is_none")]
    pub unique_items: Option<bool>,
    /// `unevaluatedItems` — applies to elements no other keyword evaluated.
    #[serde(rename = "unevaluatedItems", skip_serializing_if = "Option::is_none")]
    pub unevaluated_items: Option<Box<Schema>>,

    // --- validation: objects ---------------------------------------------
    /// `properties` — schemas by exact property name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<OrderedMap<Schema>>,
    /// `required` — property names that must be present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
    /// `patternProperties` — schemas by property-name regex.
    #[serde(rename = "patternProperties", skip_serializing_if = "Option::is_none")]
    pub pattern_properties: Option<OrderedMap<Schema>>,
    /// `additionalProperties` — applies to properties the two keywords above
    /// did not match.
    #[serde(
        rename = "additionalProperties",
        skip_serializing_if = "Option::is_none"
    )]
    pub additional_properties: Option<Box<Schema>>,
    /// `propertyNames` — every property *name* must match this schema.
    #[serde(rename = "propertyNames", skip_serializing_if = "Option::is_none")]
    pub property_names: Option<Box<Schema>>,
    /// `maxProperties` — upper bound on property count.
    #[serde(rename = "maxProperties", skip_serializing_if = "Option::is_none")]
    pub max_properties: Option<u64>,
    /// `minProperties` — lower bound on property count.
    #[serde(rename = "minProperties", skip_serializing_if = "Option::is_none")]
    pub min_properties: Option<u64>,
    /// `dependentRequired` — one property's presence requires others.
    #[serde(rename = "dependentRequired", skip_serializing_if = "Option::is_none")]
    pub dependent_required: Option<OrderedMap<Vec<String>>>,
    /// `dependentSchemas` — one property's presence applies another schema.
    #[serde(rename = "dependentSchemas", skip_serializing_if = "Option::is_none")]
    pub dependent_schemas: Option<OrderedMap<Schema>>,
    /// `unevaluatedProperties` — applies to properties no other keyword
    /// evaluated, across `$ref` and the in-place applicators.
    #[serde(
        rename = "unevaluatedProperties",
        skip_serializing_if = "Option::is_none"
    )]
    pub unevaluated_properties: Option<Box<Schema>>,

    // --- applicators ------------------------------------------------------
    /// `allOf` — every subschema must match.
    #[serde(rename = "allOf", skip_serializing_if = "Option::is_none")]
    pub all_of: Option<Vec<Schema>>,
    /// `anyOf` — at least one subschema must match.
    #[serde(rename = "anyOf", skip_serializing_if = "Option::is_none")]
    pub any_of: Option<Vec<Schema>>,
    /// `oneOf` — exactly one subschema must match.
    #[serde(rename = "oneOf", skip_serializing_if = "Option::is_none")]
    pub one_of: Option<Vec<Schema>>,
    /// `not` — the subschema must *not* match.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not: Option<Box<Schema>>,
    /// `if` — the condition for `then` / `else`. Asserts nothing on its own.
    #[serde(rename = "if", skip_serializing_if = "Option::is_none")]
    pub if_schema: Option<Box<Schema>>,
    /// `then` — applied when `if` matches.
    #[serde(rename = "then", skip_serializing_if = "Option::is_none")]
    pub then_schema: Option<Box<Schema>>,
    /// `else` — applied when `if` does not match.
    #[serde(rename = "else", skip_serializing_if = "Option::is_none")]
    pub else_schema: Option<Box<Schema>>,

    // --- content ----------------------------------------------------------
    /// `contentEncoding` — how the string decodes to octets, e.g. `base64`.
    #[serde(rename = "contentEncoding", skip_serializing_if = "Option::is_none")]
    pub content_encoding: Option<String>,
    /// `contentMediaType` — the media type of the decoded string.
    #[serde(rename = "contentMediaType", skip_serializing_if = "Option::is_none")]
    pub content_media_type: Option<String>,
    /// `contentSchema` — a schema for the decoded content.
    #[serde(rename = "contentSchema", skip_serializing_if = "Option::is_none")]
    pub content_schema: Option<Box<Schema>>,

    // --- metadata: annotations --------------------------------------------
    /// `default` — a suggested value, asserted by nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// `examples` — sample instances, asserted by nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<Value>>,
    /// `deprecated` — this location should no longer be used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,
    /// `readOnly` — the value is supplied by the authority, not the client.
    #[serde(rename = "readOnly", skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    /// `writeOnly` — the value may be sent but is never returned.
    #[serde(rename = "writeOnly", skip_serializing_if = "Option::is_none")]
    pub write_only: Option<bool>,

    // --- reusable subschemas ----------------------------------------------
    /// `$defs` — reusable subschemas, referenced by `$ref`.
    #[serde(rename = "$defs", skip_serializing_if = "Option::is_none")]
    pub defs: Option<OrderedMap<Schema>>,
}

impl SchemaObject {
    /// A schema object with no keywords set — equivalent to `{}`, which accepts
    /// every instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether no keyword is set. Such a schema asserts nothing.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

// ---------------------------------------------------------------------------
// The schema
// ---------------------------------------------------------------------------

/// A JSON Schema.
///
/// A schema is either a boolean — `true` accepts every instance, `false`
/// rejects every instance — or an object of keywords. Both forms are legal
/// everywhere a schema is expected, which is why this is an enum rather than a
/// struct.
///
/// Build one with the entry points below, then call `.build()`:
///
/// ```
/// # use ash_jsonschema_builder::Schema;
/// let schema = Schema::object()
///     .dialect()
///     .required_property("id", Schema::string().format("uuid"))
///     .additional_properties(false)
///     .build().unwrap();
///
/// assert_eq!(
///     schema.to_json(),
///     r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"id":{"type":"string","format":"uuid"}},"required":["id"],"additionalProperties":false}"#,
/// );
/// ```
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Schema {
    /// The boolean form: `true` accepts everything, `false` rejects everything.
    Bool(bool),
    /// The object form: a bag of keywords.
    Object(Box<SchemaObject>),
}

impl Schema {
    // --- entry points -----------------------------------------------------

    /// A builder with no `type` committed — for schemas that assert through
    /// references, combinators, or `const`/`enum` alone.
    pub fn builder() -> AnyBuilder {
        Builder::new()
    }

    /// A builder for `"type": "string"`.
    pub fn string() -> StringBuilder {
        Builder::of_type(SchemaType::String)
    }

    /// A builder for `"type": "number"` — any JSON number.
    pub fn number() -> NumericBuilder {
        Builder::of_type(SchemaType::Number)
    }

    /// A builder for `"type": "integer"` — a number with a zero fractional
    /// part. Same keywords as [`Schema::number`].
    pub fn integer() -> NumericBuilder {
        Builder::of_type(SchemaType::Integer)
    }

    /// A builder for `"type": "object"`.
    pub fn object() -> ObjectBuilder {
        Builder::of_type(SchemaType::Object)
    }

    /// A builder for `"type": "array"`.
    pub fn array() -> ArrayBuilder {
        Builder::of_type(SchemaType::Array)
    }

    /// A builder for `"type": "boolean"`. Note this is the *instance* type —
    /// for the boolean schema form see [`Schema::anything`] and
    /// [`Schema::nothing`].
    pub fn boolean() -> AnyBuilder {
        Builder::of_type(SchemaType::Boolean)
    }

    /// A builder for `"type": "null"`.
    pub fn null() -> AnyBuilder {
        Builder::of_type(SchemaType::Null)
    }

    /// A builder for a set of permitted types, e.g.
    /// `"type": ["string", "null"]`.
    pub fn of_types(types: impl IntoIterator<Item = SchemaType>) -> AnyBuilder {
        Builder::new().types(types)
    }

    /// A builder for a lone `$ref`.
    ///
    /// ```
    /// # use ash_jsonschema_builder::Schema;
    /// let schema = Schema::reference("#/$defs/Address").build().unwrap();
    /// assert_eq!(schema.to_json(), r##"{"$ref":"#/$defs/Address"}"##);
    /// ```
    pub fn reference(uri: impl Into<String>) -> AnyBuilder {
        Builder::new().reference(uri)
    }

    /// A builder for `allOf`.
    pub fn all_of(schemas: impl IntoIterator<Item = impl Into<Schema>>) -> AnyBuilder {
        Builder::new().all_of(schemas)
    }

    /// A builder for `anyOf`.
    pub fn any_of(schemas: impl IntoIterator<Item = impl Into<Schema>>) -> AnyBuilder {
        Builder::new().any_of(schemas)
    }

    /// A builder for `oneOf`.
    pub fn one_of(schemas: impl IntoIterator<Item = impl Into<Schema>>) -> AnyBuilder {
        Builder::new().one_of(schemas)
    }

    /// A builder for `not`.
    pub fn not(schema: impl Into<Schema>) -> AnyBuilder {
        Builder::new().not(schema)
    }

    /// The boolean schema `true`: every instance is valid.
    pub fn anything() -> Schema {
        Schema::Bool(true)
    }

    /// The boolean schema `false`: no instance is valid.
    pub fn nothing() -> Schema {
        Schema::Bool(false)
    }

    // --- inspection -------------------------------------------------------

    /// The keyword bag, for the object form.
    pub fn as_object(&self) -> Option<&SchemaObject> {
        match self {
            Self::Object(object) => Some(object),
            Self::Bool(_) => None,
        }
    }

    /// The boolean, for the boolean form.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            Self::Object(_) => None,
        }
    }

    // --- validation -------------------------------------------------------

    /// Checks this schema against the embedded JSON Schema 2020-12
    /// meta-schema, which [`Builder::build`](crate::Builder::build) does for
    /// you.
    ///
    /// The walk is depth-first over the document, so every subschema is checked
    /// in place and a violation carries the JSON Pointer that locates it.
    ///
    /// ```
    /// # use ash_jsonschema_builder::Schema;
    /// let schema = Schema::string().min_length(1).build_unchecked();
    /// assert!(schema.validate().is_ok());
    /// ```
    pub fn validate(&self) -> Result<(), crate::error::SchemaError> {
        crate::validate::validate_document(&self.to_value())
    }

    /// Freezes this schema so it can never be modified again.
    ///
    /// [`Builder::build_frozen`](crate::Builder::build_frozen) is the usual way
    /// to reach a [`FrozenSchema`](crate::FrozenSchema); this is for one you
    /// already hold.
    ///
    /// ```
    /// # use ash_jsonschema_builder::Schema;
    /// let frozen = Schema::string().build_unchecked().freeze();
    /// assert_eq!(frozen.to_json(), r#"{"type":"string"}"#);
    /// ```
    pub fn freeze(self) -> crate::frozen::FrozenSchema {
        crate::frozen::FrozenSchema::new(self)
    }

    // --- rendering --------------------------------------------------------

    /// Renders the schema as a [`serde_json::Value`].
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("a schema is always representable as JSON")
    }

    /// Renders the schema as compact JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a schema is always representable as JSON")
    }

    /// Renders the schema as indented JSON.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("a schema is always representable as JSON")
    }
}

impl From<bool> for Schema {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<SchemaObject> for Schema {
    fn from(value: SchemaObject) -> Self {
        Self::Object(Box::new(value))
    }
}

impl std::fmt::Display for Schema {
    /// Writes the indented JSON form.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_json_pretty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_do_not_acquire_a_fractional_part() {
        assert_eq!(serde_json::to_string(&Num::from(7u8)).unwrap(), "7");
        assert_eq!(serde_json::to_string(&Num::from(-7i32)).unwrap(), "-7");
        assert_eq!(serde_json::to_string(&Num::from(0.5f64)).unwrap(), "0.5");
    }

    #[test]
    fn non_finite_floats_are_rejected() {
        assert_eq!(Num::try_from_f64(f64::NAN), None);
        assert_eq!(Num::try_from_f64(f64::INFINITY), None);
        assert!(Num::try_from_f64(1.5).is_some());
    }

    #[test]
    #[should_panic(expected = "no representation")]
    fn converting_a_non_finite_float_panics() {
        let _ = Num::from(f64::NAN);
    }

    #[test]
    fn a_single_type_renders_bare_and_a_set_renders_as_an_array() {
        let one = TypeSet::One(SchemaType::String);
        assert_eq!(serde_json::to_string(&one).unwrap(), r#""string""#);

        let many = TypeSet::Many(vec![SchemaType::String, SchemaType::Null]);
        assert_eq!(
            serde_json::to_string(&many).unwrap(),
            r#"["string","null"]"#
        );
    }

    #[test]
    fn inserting_widens_a_single_type_and_ignores_duplicates() {
        let mut types = TypeSet::One(SchemaType::String);
        types.insert(SchemaType::String);
        assert_eq!(types, TypeSet::One(SchemaType::String));

        types.insert(SchemaType::Null);
        assert_eq!(
            types,
            TypeSet::Many(vec![SchemaType::String, SchemaType::Null])
        );

        types.insert(SchemaType::Null);
        assert_eq!(
            types,
            TypeSet::Many(vec![SchemaType::String, SchemaType::Null])
        );
    }

    #[test]
    fn collecting_one_type_yields_the_single_form() {
        let one: TypeSet = [SchemaType::Integer].into_iter().collect();
        assert_eq!(one, TypeSet::One(SchemaType::Integer));

        let deduped: TypeSet = [SchemaType::Integer, SchemaType::Integer]
            .into_iter()
            .collect();
        assert_eq!(deduped, TypeSet::One(SchemaType::Integer));
    }

    #[test]
    fn boolean_schemas_render_as_bare_booleans() {
        assert_eq!(Schema::anything().to_json(), "true");
        assert_eq!(Schema::nothing().to_json(), "false");
        assert_eq!(Schema::anything().as_bool(), Some(true));
        assert_eq!(Schema::anything().as_object(), None);
    }

    #[test]
    fn an_empty_keyword_bag_renders_as_an_empty_object() {
        let schema = Schema::from(SchemaObject::new());
        assert!(schema.as_object().unwrap().is_empty());
        assert_eq!(schema.to_json(), "{}");
    }

    #[test]
    fn display_renders_the_indented_form() {
        let schema = Schema::string().build().unwrap();
        assert_eq!(schema.to_string(), "{\n  \"type\": \"string\"\n}");
    }
}

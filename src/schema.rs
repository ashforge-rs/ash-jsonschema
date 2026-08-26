//! The schema document itself: [`Schema`], the keyword bag [`SchemaObject`] it
//! wraps, and the small value types the keywords are written in.
//!
//! This module is the *data*. Nothing here enforces a construction order or a
//! keyword's meaning — that is the [`builder`](crate::builder) module's job.
//! Everything is public so a caller who needs a keyword combination the
//! builders do not expose can assemble a [`SchemaObject`] by hand.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
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
/// # use ash_jsonschema::Num;
/// assert_eq!(serde_json::to_string(&Num::from(3)).unwrap(), "3");
/// assert_eq!(serde_json::to_string(&Num::from(2.5)).unwrap(), "2.5");
/// ```
#[derive(Clone, Debug, PartialEq, Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

impl SchemaType {
    /// The seven type names, in the order the specification lists them.
    pub const NAMES: [&'static str; 7] = [
        "null", "boolean", "object", "array", "number", "integer", "string",
    ];

    /// The type a name denotes, or `None` if it denotes none of them.
    ///
    /// The names are exactly as the specification writes them, so the match is
    /// case-sensitive. [`FromStr`](std::str::FromStr) is implemented too, for
    /// when a `Result` suits the caller better than an `Option`.
    ///
    /// ```
    /// # use ash_jsonschema::SchemaType;
    /// assert_eq!(SchemaType::parse("integer"), Some(SchemaType::Integer));
    /// assert_eq!(SchemaType::parse("Integer"), None);
    ///
    /// let parsed: SchemaType = "string".parse().unwrap();
    /// assert_eq!(parsed, SchemaType::String);
    /// ```
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "null" => Some(Self::Null),
            "boolean" => Some(Self::Boolean),
            "object" => Some(Self::Object),
            "array" => Some(Self::Array),
            "number" => Some(Self::Number),
            "integer" => Some(Self::Integer),
            "string" => Some(Self::String),
            _ => None,
        }
    }
}

impl std::fmt::Display for SchemaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A name that denotes none of the seven types the `type` keyword accepts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownSchemaType(String);

impl UnknownSchemaType {
    /// The name that was not recognised.
    pub fn name(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for UnknownSchemaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "`{}` is not a JSON Schema type; expected one of {}",
            self.0,
            SchemaType::NAMES.join(", "),
        )
    }
}

impl std::error::Error for UnknownSchemaType {}

impl std::str::FromStr for SchemaType {
    type Err = UnknownSchemaType;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::parse(name).ok_or_else(|| UnknownSchemaType(name.to_owned()))
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

impl<'de> Deserialize<'de> for TypeSet {
    /// Reads either the bare form or the array form, naming the seven legal
    /// types when it reads neither — the derived `untagged` reader would only
    /// say that no variant matched.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TypeSetVisitor;

        impl<'de> serde::de::Visitor<'de> for TypeSetVisitor {
            type Value = TypeSet;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(
                    "a type name (\"null\", \"boolean\", \"object\", \"array\", \"number\",                      \"integer\", \"string\") or an array of them",
                )
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<TypeSet, E> {
                SchemaType::parse(value)
                    .map(TypeSet::One)
                    .ok_or_else(|| E::unknown_variant(value, &SchemaType::NAMES))
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<TypeSet, A::Error> {
                let mut types = Vec::new();
                while let Some(name) = access.next_element::<String>()? {
                    let ty = SchemaType::parse(&name).ok_or_else(|| {
                        serde::de::Error::unknown_variant(&name, &SchemaType::NAMES)
                    })?;
                    types.push(ty);
                }
                Ok(TypeSet::Many(types))
            }
        }

        deserializer.deserialize_any(TypeSetVisitor)
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

/// Every keyword name the 2020-12 dialect defines, as it is written in a
/// document.
///
/// A name in this list has a typed field on [`SchemaObject`]; a name outside it
/// belongs in [`SchemaObject::extensions`]. The two are kept apart so a
/// keyword can never be written twice into one rendered object.
pub const DIALECT_KEYWORDS: [&str; 57] = [
    "$schema",
    "$id",
    "$anchor",
    "$dynamicAnchor",
    "$ref",
    "$dynamicRef",
    "$vocabulary",
    "$comment",
    "title",
    "description",
    "type",
    "enum",
    "const",
    "multipleOf",
    "maximum",
    "exclusiveMaximum",
    "minimum",
    "exclusiveMinimum",
    "maxLength",
    "minLength",
    "pattern",
    "format",
    "prefixItems",
    "items",
    "contains",
    "maxContains",
    "minContains",
    "maxItems",
    "minItems",
    "uniqueItems",
    "unevaluatedItems",
    "properties",
    "required",
    "patternProperties",
    "additionalProperties",
    "propertyNames",
    "maxProperties",
    "minProperties",
    "dependentRequired",
    "dependentSchemas",
    "unevaluatedProperties",
    "allOf",
    "anyOf",
    "oneOf",
    "not",
    "if",
    "then",
    "else",
    "contentEncoding",
    "contentMediaType",
    "contentSchema",
    "default",
    "examples",
    "deprecated",
    "readOnly",
    "writeOnly",
    "$defs",
];

/// Whether `name` is a keyword the 2020-12 dialect defines, and so has a typed
/// field rather than belonging in [`SchemaObject::extensions`].
///
/// ```
/// # use ash_jsonschema::schema::is_dialect_keyword;
/// assert!(is_dialect_keyword("minLength"));
/// assert!(!is_dialect_keyword("x-internal"));
/// ```
pub fn is_dialect_keyword(name: &str) -> bool {
    DIALECT_KEYWORDS.contains(&name)
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
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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
    ///
    /// `Some(Value::Null)` is the keyword written as `"const": null`, which
    /// accepts only `null`; `None` is the keyword being absent, which accepts
    /// anything. JSON gives those very different meanings, so the two are read
    /// back distinctly rather than both landing on `None`.
    #[serde(
        rename = "const",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_or_absent"
    )]
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
    #[serde(
        rename = "maxLength",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "count",
        default
    )]
    pub max_length: Option<u64>,
    /// `minLength` — in Unicode code points, not bytes.
    #[serde(
        rename = "minLength",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "count",
        default
    )]
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
    #[serde(
        rename = "maxContains",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "count",
        default
    )]
    pub max_contains: Option<u64>,
    /// `minContains` — lower bound on `contains` matches.
    #[serde(
        rename = "minContains",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "count",
        default
    )]
    pub min_contains: Option<u64>,
    /// `maxItems` — upper bound on element count.
    #[serde(
        rename = "maxItems",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "count",
        default
    )]
    pub max_items: Option<u64>,
    /// `minItems` — lower bound on element count.
    #[serde(
        rename = "minItems",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "count",
        default
    )]
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
    #[serde(
        rename = "maxProperties",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "count",
        default
    )]
    pub max_properties: Option<u64>,
    /// `minProperties` — lower bound on property count.
    #[serde(
        rename = "minProperties",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "count",
        default
    )]
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
    ///
    /// As with [`constant`](Self::constant), an explicit `null` is kept as
    /// `Some(Value::Null)` rather than being conflated with the keyword's
    /// absence.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "present_or_absent")]
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

    // --- everything else --------------------------------------------------
    /// Keywords outside the 2020-12 dialect, kept verbatim.
    ///
    /// JSON Schema requires an implementation to ignore keywords it does not
    /// recognise rather than reject them, which is what makes extension
    /// vocabularies — OpenAPI's `discriminator`, an `x-`-prefixed annotation,
    /// a private validator's own attribute — legal in a schema document. They
    /// are collected here so parsing a document and rendering it back out
    /// does not quietly discard them.
    ///
    /// These render last, after every dialect keyword, each under the name it
    /// was written with.
    #[serde(flatten)]
    pub extensions: OrderedMap<Value>,
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

/// Reads a keyword whose value may legitimately be `null`.
///
/// `Option<Value>` normally collapses the two cases: serde hands an explicit
/// `null` to `Option::deserialize`, which reads it as `None` — indistinguishable
/// from the keyword not being there. For `const` and `default` that difference
/// is the whole meaning, so this reads the value first and wraps it after.
fn present_or_absent<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

/// Reads a count keyword — `minItems`, `maxLength`, and their kind.
///
/// JSON has one number type, so `1.0` and `1` are the same value written two
/// ways, and the meta-schema's `nonNegativeInteger` admits both. Serde's `u64`
/// does not, which would reject a schema the specification calls valid — so
/// an integral float is accepted and anything with a real fraction is not.
fn count<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    use serde::de::Error;

    match Option::<serde_json::Number>::deserialize(deserializer)? {
        None => Ok(None),
        Some(number) => {
            if let Some(value) = number.as_u64() {
                return Ok(Some(value));
            }
            // `2.0` counts; `2.5` and `-1` do not.
            match number.as_f64() {
                Some(value) if value.fract() == 0.0 && value >= 0.0 && value <= u64::MAX as f64 => {
                    Ok(Some(value as u64))
                }
                _ => Err(D::Error::custom(format!(
                    "expected a non-negative integer, found {number}"
                ))),
            }
        }
    }
}

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
/// # use ash_jsonschema::Schema;
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
    /// # use ash_jsonschema::Schema;
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

    // --- parsing ----------------------------------------------------------

    /// Parses a schema from JSON text.
    ///
    /// This reads the document; it does not check that it *is* a schema — call
    /// [`validate`](Self::validate) for that, or use
    /// [`parse_checked`](Self::parse_checked) to do both at once. Keywords
    /// outside the 2020-12 dialect are kept in
    /// [`SchemaObject::extensions`] rather than discarded, so a document
    /// parsed and rendered back out keeps everything it arrived with.
    ///
    /// ```
    /// # use ash_jsonschema::Schema;
    /// let schema = Schema::parse(r#"{"type":"string","minLength":1}"#)?;
    /// assert_eq!(schema.as_object().unwrap().min_length, Some(1));
    /// # Ok::<(), serde_json::Error>(())
    /// ```
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Parses a schema from JSON text and checks it against the meta-schema.
    ///
    /// The two failure modes stay distinct: [`ParseError::Syntax`](crate::ParseError::Syntax) means the
    /// text was not JSON at all, [`ParseError::Invalid`](crate::ParseError::Invalid) means it parsed but is
    /// not a well-formed 2020-12 schema.
    ///
    /// ```
    /// # use ash_jsonschema::Schema;
    /// let schema = Schema::parse_checked(r#"{"type":"string"}"#).unwrap();
    /// assert!(schema.as_object().is_some());
    ///
    /// let bad = Schema::parse_checked(r#"{"$anchor":"not an anchor"}"#).unwrap_err();
    /// assert!(bad.to_string().contains("$anchor"));
    /// ```
    pub fn parse_checked(json: &str) -> Result<Self, crate::error::ParseError> {
        let schema = Self::parse(json)?;
        schema.validate()?;
        Ok(schema)
    }

    /// Reads a schema out of an already-parsed [`serde_json::Value`].
    pub fn from_value(value: Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }

    // --- inspection -------------------------------------------------------

    /// The keyword bag, for the object form.
    pub fn as_object(&self) -> Option<&SchemaObject> {
        match self {
            Self::Object(object) => Some(object),
            Self::Bool(_) => None,
        }
    }

    /// The keyword bag, mutably, for the object form.
    pub fn as_object_mut(&mut self) -> Option<&mut SchemaObject> {
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

    /// The permitted instance type(s), if `type` is set.
    pub fn schema_type(&self) -> Option<&TypeSet> {
        self.as_object().and_then(|object| object.types.as_ref())
    }

    /// The subschema under `properties` for `name`, if any.
    ///
    /// ```
    /// # use ash_jsonschema::Schema;
    /// let schema = Schema::object().property("id", Schema::string()).build()?;
    /// assert_eq!(schema.property("id").and_then(Schema::as_bool), None);
    /// assert!(schema.property("id").is_some());
    /// assert!(schema.property("missing").is_none());
    /// # Ok::<(), ash_jsonschema::SchemaError>(())
    /// ```
    pub fn property(&self, name: &str) -> Option<&Schema> {
        self.as_object()?.properties.as_ref()?.get(name)
    }

    /// Whether `name` is listed in `required`.
    pub fn requires(&self, name: &str) -> bool {
        self.as_object()
            .and_then(|object| object.required.as_ref())
            .is_some_and(|required| required.iter().any(|entry| entry == name))
    }

    /// The subschema under `$defs` for `name`, if any.
    pub fn def(&self, name: &str) -> Option<&Schema> {
        self.as_object()?.defs.as_ref()?.get(name)
    }

    /// Resolves a JSON Pointer against this schema, as
    /// [`Violation::location`](crate::Violation::location) reports one.
    ///
    /// The empty pointer is the root. This resolves against the *rendered*
    /// document, so it reaches every keyword under the name it is written with,
    /// including the ones inside [`extensions`](SchemaObject::extensions).
    ///
    /// The value is cloned out of a rendering of this schema, so resolving many
    /// pointers over one schema is better served by holding a
    /// [`to_value`](Self::to_value) once and using
    /// [`serde_json::Value::pointer`] on it.
    ///
    /// ```
    /// # use ash_jsonschema::Schema;
    /// let schema = Schema::object()
    ///     .property("id", Schema::string().min_length(1))
    ///     .build()?;
    ///
    /// assert_eq!(schema.pointer("/properties/id/minLength"), Some(serde_json::json!(1)));
    /// assert_eq!(schema.pointer("/properties/nope"), None);
    /// # Ok::<(), ash_jsonschema::SchemaError>(())
    /// ```
    pub fn pointer(&self, pointer: &str) -> Option<Value> {
        self.to_value().pointer(pointer).cloned()
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
    /// # use ash_jsonschema::Schema;
    /// let schema = Schema::string().min_length(1).build_unchecked();
    /// assert!(schema.validate().is_ok());
    /// ```
    pub fn validate(&self) -> Result<(), crate::error::SchemaError> {
        crate::validate::validate_document(&self.to_value())
    }

    /// Returns an owned copy of this schema.
    ///
    /// The copy is detached: editing it leaves this one untouched, and vice
    /// versa. This is what [`Clone`] does, named for the intent — it is the
    /// same call whether you hold a [`Schema`], a
    /// [`FrozenSchema`](crate::FrozenSchema), or a builder, so a caller that
    /// wants a copy out need not know which it has.
    ///
    /// ```
    /// # use ash_jsonschema::Schema;
    /// let schema = Schema::object().title("Original").build()?;
    ///
    /// let mut copy = schema.export();
    /// copy.as_object_mut().unwrap().title = Some("Mine".into());
    ///
    /// assert_eq!(copy.as_object().unwrap().title.as_deref(), Some("Mine"));
    /// assert_eq!(schema.as_object().unwrap().title.as_deref(), Some("Original"));
    /// # Ok::<(), ash_jsonschema::SchemaError>(())
    /// ```
    pub fn export(&self) -> Schema {
        self.clone()
    }

    /// Returns an owned copy of the subschema at `pointer`.
    ///
    /// The pointer is the one [`pointer`](Self::pointer) takes and a
    /// [`Violation`](crate::Violation) reports — the empty string is the root.
    /// Unlike `pointer`, what comes back is a [`Schema`] rather than a bare
    /// [`Value`], so a subtree can be lifted out and used as a schema in its
    /// own right.
    ///
    /// Returns `None` if the pointer names nothing, or names a place that is
    /// not a schema — `/properties` is a map of subschemas, not one itself.
    ///
    /// ```
    /// # use ash_jsonschema::Schema;
    /// let schema = Schema::object()
    ///     .property("address", Schema::object().required(["city"]))
    ///     .build()?;
    ///
    /// let address = schema.export_at("/properties/address").unwrap();
    /// assert_eq!(address.to_json(), r#"{"type":"object","required":["city"]}"#);
    ///
    /// assert!(schema.export_at("/properties/missing").is_none());
    /// assert!(schema.export_at("/properties").is_none());  // a map, not a schema
    /// # Ok::<(), ash_jsonschema::SchemaError>(())
    /// ```
    pub fn export_at(&self, pointer: &str) -> Option<Schema> {
        if pointer.is_empty() {
            return Some(self.export());
        }
        // Walking to the subschema keeps this honest about what counts as one:
        // a pointer into `properties` itself lands on a map of schemas, which
        // is not a schema, and the walk never visits it.
        let document = self.to_value();
        let mut found = None;
        crate::walk::walk_subschemas(&document, |at, node| {
            if at == pointer {
                found = Some(node.clone());
            }
        });
        Schema::from_value(found?).ok()
    }

    /// Freezes this schema so it can never be modified again.
    ///
    /// [`Builder::build_frozen`](crate::Builder::build_frozen) is the usual way
    /// to reach a [`FrozenSchema`](crate::FrozenSchema); this is for one you
    /// already hold.
    ///
    /// ```
    /// # use ash_jsonschema::Schema;
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

impl<'de> Deserialize<'de> for Schema {
    /// Reads either schema form, keeping the reason a bad document was
    /// rejected.
    ///
    /// The derived `untagged` reader would try each variant and, on failing
    /// both, report only that nothing matched — losing the actual mistake and
    /// its position. Dispatching on the token instead means a boolean is read
    /// as a boolean, anything else is read as a keyword bag, and the error
    /// that comes back is the one serde raised for the offending keyword.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SchemaVisitor;

        impl<'de> serde::de::Visitor<'de> for SchemaVisitor {
            type Value = Schema;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON Schema: an object of keywords, or a boolean")
            }

            fn visit_bool<E>(self, value: bool) -> Result<Schema, E> {
                Ok(Schema::Bool(value))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                access: A,
            ) -> Result<Schema, A::Error> {
                let object = SchemaObject::deserialize(
                    serde::de::value::MapAccessDeserializer::new(access),
                )?;
                Ok(Schema::Object(Box::new(object)))
            }
        }

        deserializer.deserialize_any(SchemaVisitor)
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
    fn a_type_name_parses_both_ways_and_reports_what_it_expected() {
        use std::str::FromStr;

        assert_eq!(SchemaType::parse("integer"), Some(SchemaType::Integer));
        assert_eq!(SchemaType::parse("Integer"), None);

        let error = SchemaType::from_str("strng").unwrap_err();
        assert_eq!(error.name(), "strng");
        assert!(error.to_string().contains("is not a JSON Schema type"));
        assert!(error.to_string().contains("string"));

        for name in SchemaType::NAMES {
            assert_eq!(SchemaType::parse(name).unwrap().as_str(), name);
        }
    }

    #[test]
    fn boolean_schemas_render_as_bare_booleans() {
        assert_eq!(Schema::anything().to_json(), "true");
        assert_eq!(Schema::nothing().to_json(), "false");
        assert_eq!(Schema::anything().as_bool(), Some(true));
        assert_eq!(Schema::anything().as_object(), None);
    }

    /// The constant and the struct must not drift apart: every name in the list
    /// has to be a key the rendered object actually uses, and every key the
    /// rendered object uses has to be in the list.
    #[test]
    fn the_dialect_keyword_list_matches_what_the_struct_renders() {
        // A schema object with every keyword set, so all 57 render.
        let mut object = SchemaObject::new();
        object.schema = Some(DIALECT.into());
        object.id = Some("https://example.com".into());
        object.anchor = Some("a".into());
        object.dynamic_anchor = Some("a".into());
        object.reference = Some("#".into());
        object.dynamic_ref = Some("#".into());
        object.vocabulary = Some(OrderedMap::new());
        object.comment = Some("c".into());
        object.title = Some("t".into());
        object.description = Some("d".into());
        object.types = Some(TypeSet::One(SchemaType::Object));
        object.enumeration = Some(Vec::new());
        object.constant = Some(Value::Null);
        object.multiple_of = Some(Num::from(1));
        object.maximum = Some(Num::from(1));
        object.exclusive_maximum = Some(Num::from(1));
        object.minimum = Some(Num::from(1));
        object.exclusive_minimum = Some(Num::from(1));
        object.max_length = Some(1);
        object.min_length = Some(1);
        object.pattern = Some("a".into());
        object.format = Some(Format::Email);
        object.prefix_items = Some(Vec::new());
        object.items = Some(Box::new(Schema::Bool(true)));
        object.contains = Some(Box::new(Schema::Bool(true)));
        object.max_contains = Some(1);
        object.min_contains = Some(1);
        object.max_items = Some(1);
        object.min_items = Some(1);
        object.unique_items = Some(true);
        object.unevaluated_items = Some(Box::new(Schema::Bool(true)));
        object.properties = Some(OrderedMap::new());
        object.required = Some(Vec::new());
        object.pattern_properties = Some(OrderedMap::new());
        object.additional_properties = Some(Box::new(Schema::Bool(true)));
        object.property_names = Some(Box::new(Schema::Bool(true)));
        object.max_properties = Some(1);
        object.min_properties = Some(1);
        object.dependent_required = Some(OrderedMap::new());
        object.dependent_schemas = Some(OrderedMap::new());
        object.unevaluated_properties = Some(Box::new(Schema::Bool(true)));
        object.all_of = Some(Vec::new());
        object.any_of = Some(Vec::new());
        object.one_of = Some(Vec::new());
        object.not = Some(Box::new(Schema::Bool(true)));
        object.if_schema = Some(Box::new(Schema::Bool(true)));
        object.then_schema = Some(Box::new(Schema::Bool(true)));
        object.else_schema = Some(Box::new(Schema::Bool(true)));
        object.content_encoding = Some("base64".into());
        object.content_media_type = Some("application/json".into());
        object.content_schema = Some(Box::new(Schema::Bool(true)));
        object.default = Some(Value::Null);
        object.examples = Some(Vec::new());
        object.deprecated = Some(true);
        object.read_only = Some(true);
        object.write_only = Some(true);
        object.defs = Some(OrderedMap::new());

        let rendered = Schema::from(object).to_value();
        let keys: Vec<&str> = rendered
            .as_object()
            .expect("the object form renders as a JSON object")
            .keys()
            .map(String::as_str)
            .collect();

        for key in &keys {
            assert!(
                is_dialect_keyword(key),
                "`{key}` renders but is missing from DIALECT_KEYWORDS",
            );
        }
        for keyword in DIALECT_KEYWORDS {
            assert!(
                keys.contains(&keyword),
                "DIALECT_KEYWORDS lists `{keyword}`, which the struct never renders",
            );
        }
        assert_eq!(keys.len(), DIALECT_KEYWORDS.len());
    }

    #[test]
    fn an_empty_keyword_bag_renders_as_an_empty_object() {
        let schema = Schema::from(SchemaObject::new());
        assert!(schema.as_object().unwrap().is_empty());
        assert_eq!(schema.to_json(), "{}");
    }

    #[test]
    fn an_exported_copy_is_detached_from_the_original() {
        let schema = Schema::object().title("Original").build().unwrap();
        let mut copy = schema.export();

        copy.as_object_mut().unwrap().title = Some("Mine".into());

        assert_eq!(copy.as_object().unwrap().title.as_deref(), Some("Mine"));
        assert_eq!(
            schema.as_object().unwrap().title.as_deref(),
            Some("Original")
        );
    }

    #[test]
    fn exporting_a_subschema_yields_a_schema_in_its_own_right() {
        let schema = Schema::object()
            .property("address", Schema::object().required(["city"]))
            .property("tags", Schema::array().items(Schema::string()))
            .build()
            .unwrap();

        let address = schema.export_at("/properties/address").unwrap();
        assert_eq!(
            address.to_json(),
            r#"{"type":"object","required":["city"]}"#
        );

        // Nested positions resolve too.
        let item = schema.export_at("/properties/tags/items").unwrap();
        assert_eq!(item, Schema::string().build().unwrap());

        // And the copy is detached.
        let mut mine = address.export();
        mine.as_object_mut().unwrap().title = Some("Address".into());
        assert_eq!(schema.export_at("/properties/address").unwrap(), address);
    }

    #[test]
    fn exporting_the_empty_pointer_is_the_whole_schema() {
        let schema = Schema::string().min_length(2).build().unwrap();
        assert_eq!(schema.export_at("").unwrap(), schema);
    }

    #[test]
    fn exporting_a_place_that_is_not_a_schema_yields_nothing() {
        let schema = Schema::object()
            .property("a", Schema::string())
            .build()
            .unwrap();

        // `properties` is a map of schemas, not a schema.
        assert_eq!(schema.export_at("/properties"), None);
        // A keyword whose value is a plain scalar is not one either.
        assert_eq!(schema.export_at("/type"), None);
        // Nor is something absent.
        assert_eq!(schema.export_at("/properties/missing"), None);
    }

    #[test]
    fn a_boolean_subschema_exports_as_one() {
        let schema = Schema::object()
            .additional_properties(false)
            .build()
            .unwrap();

        assert_eq!(
            schema.export_at("/additionalProperties"),
            Some(Schema::Bool(false)),
        );
    }

    #[test]
    fn display_renders_the_indented_form() {
        let schema = Schema::string().build().unwrap();
        assert_eq!(schema.to_string(), "{\n  \"type\": \"string\"\n}");
    }
}

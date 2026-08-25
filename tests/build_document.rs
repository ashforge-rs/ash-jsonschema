//! End-to-end: build documents of the kind a real caller would, and assert on
//! the exact JSON that comes out.

use ash_jsonschema_builder::{DIALECT, Format, Schema, SchemaType};

/// A realistic root document: dialect, identity, reusable `$defs`, a mix of
/// required and optional properties, and a closed object.
#[test]
fn a_root_document_renders_every_vocabulary_it_uses() {
    let schema = Schema::object()
        .dialect()
        .id("https://example.com/schemas/order")
        .title("Order")
        .description("A customer order.")
        .def(
            "Money",
            Schema::object()
                .required_property("amount", Schema::integer().minimum(0))
                .required_property("currency", Schema::string().pattern("^[A-Z]{3}$").length(3))
                .additional_properties(false),
        )
        .required_property("id", Schema::string().format(Format::Uuid))
        .required_property("placed_at", Schema::string().format(Format::DateTime))
        .required_property("total", Schema::reference("#/$defs/Money"))
        .property(
            "items",
            Schema::array()
                .items(Schema::object().required(["sku", "quantity"]))
                .min_items(1)
                .unique_items(true),
        )
        .property("note", Schema::string().max_length(280).nullable())
        .additional_properties(false)
        .build()
        .unwrap();

    let expected = serde_json::json!({
        "$schema": DIALECT,
        "$id": "https://example.com/schemas/order",
        "title": "Order",
        "description": "A customer order.",
        "type": "object",
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "placed_at": { "type": "string", "format": "date-time" },
            "total": { "$ref": "#/$defs/Money" },
            "items": {
                "type": "array",
                "items": { "type": "object", "required": ["sku", "quantity"] },
                "minItems": 1,
                "uniqueItems": true
            },
            "note": { "type": ["string", "null"], "maxLength": 280 }
        },
        "required": ["id", "placed_at", "total"],
        "additionalProperties": false,
        "$defs": {
            "Money": {
                "type": "object",
                "properties": {
                    "amount": { "type": "integer", "minimum": 0 },
                    "currency": { "type": "string", "maxLength": 3, "minLength": 3, "pattern": "^[A-Z]{3}$" }
                },
                "required": ["amount", "currency"],
                "additionalProperties": false
            }
        }
    });

    assert_eq!(schema.to_value(), expected);
}

/// `if`/`then`/`else` over a discriminator, with `unevaluatedProperties`
/// closing the composed result — the thing `additionalProperties` cannot do.
#[test]
fn a_conditional_document_composes_through_applicators() {
    let schema = Schema::object()
        .required_property("kind", Schema::string().enumeration(["circle", "rect"]))
        .if_then_else(
            Schema::object().property("kind", Schema::builder().constant("circle")),
            Schema::object().required_property("radius", Schema::number().exclusive_minimum(0)),
            Schema::object()
                .required_property("width", Schema::number().exclusive_minimum(0))
                .required_property("height", Schema::number().exclusive_minimum(0)),
        )
        .unevaluated_properties(false)
        .build()
        .unwrap();

    let expected = serde_json::json!({
        "type": "object",
        "properties": { "kind": { "type": "string", "enum": ["circle", "rect"] } },
        "required": ["kind"],
        "unevaluatedProperties": false,
        "if": { "type": "object", "properties": { "kind": { "const": "circle" } } },
        "then": {
            "type": "object",
            "properties": { "radius": { "type": "number", "exclusiveMinimum": 0 } },
            "required": ["radius"]
        },
        "else": {
            "type": "object",
            "properties": {
                "width": { "type": "number", "exclusiveMinimum": 0 },
                "height": { "type": "number", "exclusiveMinimum": 0 }
            },
            "required": ["width", "height"]
        }
    });

    assert_eq!(schema.to_value(), expected);
}

/// A tuple: positional `prefixItems`, then `items: false` to forbid a tail.
#[test]
fn a_tuple_is_a_prefix_with_a_closed_tail() {
    let schema = Schema::array()
        .title("Coordinate")
        .prefix_items([
            Schema::number().range(-90, 90).description("latitude"),
            Schema::number().range(-180, 180).description("longitude"),
        ])
        .items(false)
        .build()
        .unwrap();

    assert_eq!(
        schema.to_json(),
        concat!(
            r#"{"title":"Coordinate","type":"array","#,
            r#""prefixItems":[{"description":"latitude","type":"number","maximum":90,"minimum":-90},"#,
            r#"{"description":"longitude","type":"number","maximum":180,"minimum":-180}],"#,
            r#""items":false}"#,
        ),
    );
}

/// Combinators build from a bare builder, and take any mix of subschema forms.
#[test]
fn combinators_accept_builders_schemas_and_bare_booleans() {
    let schema = Schema::one_of([
        Schema::string().build().unwrap(),
        Schema::integer().build().unwrap(),
        Schema::Bool(true),
    ])
    .not(false)
    .build()
    .unwrap();

    assert_eq!(
        schema.to_json(),
        r#"{"oneOf":[{"type":"string"},{"type":"integer"},true],"not":false}"#,
    );
}

/// A schema that both references and refines — legal in 2020-12, where `$ref`
/// no longer displaces its siblings.
#[test]
fn a_ref_keeps_its_sibling_keywords() {
    let schema = Schema::builder()
        .reference("#/$defs/Base")
        .description("A base, narrowed.")
        .types([SchemaType::Object])
        .build()
        .unwrap();

    assert_eq!(
        schema.to_json(),
        r##"{"$ref":"#/$defs/Base","description":"A base, narrowed.","type":"object"}"##,
    );
}

/// The keyword bag is reachable, so a caller can inspect or patch what the
/// builder produced.
#[test]
fn a_built_schema_can_be_inspected_and_patched() {
    let mut keywords = Schema::object()
        .property("a", Schema::string())
        .into_object();

    assert_eq!(keywords.properties.as_ref().unwrap().len(), 1);
    keywords.title = Some("Patched".into());

    let schema = Schema::from(keywords);
    assert_eq!(
        schema.as_object().unwrap().title.as_deref(),
        Some("Patched")
    );
}

//! The macros against the builder chains they stand for.
//!
//! A macro that produced a *slightly* different document than the calls it
//! replaces would be worse than no macro at all, so these compare the two
//! directly rather than asserting on the result in isolation.

use ash_jsonschema::{Format, Schema, defs, object, one_of_tagged};

/// The shape the `order` example builds, written both ways.
#[test]
fn a_realistic_document_is_identical_written_either_way() {
    let by_macro = defs! {
        object! {
            required "id": Schema::string().format(Format::Uuid),
            required "total": Schema::reference("#/$defs/Money"),
            "items": Schema::array()
                .items(object! {
                    required "sku": Schema::string().min_length(1),
                    required "quantity": Schema::integer().minimum(1),
                }.closed())
                .min_items(1),
            "note": Schema::string().max_length(280).nullable(),
        }
        .dialect()
        .id("https://example.com/schemas/order")
        .title("Order")
        .closed();

        "Money": object! {
            required "amount": Schema::integer().minimum(0),
            required "currency": Schema::string().length(3),
        }.closed(),
    }
    .build()
    .unwrap();

    let by_hand = Schema::object()
        .dialect()
        .id("https://example.com/schemas/order")
        .title("Order")
        .def(
            "Money",
            Schema::object()
                .required_property("amount", Schema::integer().minimum(0))
                .required_property("currency", Schema::string().length(3))
                .additional_properties(false),
        )
        .required_property("id", Schema::string().format(Format::Uuid))
        .required_property("total", Schema::reference("#/$defs/Money"))
        .property(
            "items",
            Schema::array()
                .items(
                    Schema::object()
                        .required_property("sku", Schema::string().min_length(1))
                        .required_property("quantity", Schema::integer().minimum(1))
                        .additional_properties(false),
                )
                .min_items(1),
        )
        .property("note", Schema::string().max_length(280).nullable())
        .additional_properties(false)
        .build()
        .unwrap();

    assert_eq!(by_macro.to_json(), by_hand.to_json());
}

/// A tagged union is the shape most easily got subtly wrong by hand, so the
/// macro's output is compared against it written out in full.
#[test]
fn a_tagged_union_matches_the_chain_it_stands_for() {
    let by_macro = one_of_tagged! {
        "kind";
        "circle" => object! { required "radius": Schema::number() },
        "rect" => object! {
            required "width": Schema::number(),
            required "height": Schema::number(),
        },
    }
    .build()
    .unwrap();

    let by_hand = Schema::one_of([
        Schema::from(
            Schema::object()
                .required_property("radius", Schema::number())
                .required_property("kind", Schema::builder().constant("circle")),
        ),
        Schema::from(
            Schema::object()
                .required_property("width", Schema::number())
                .required_property("height", Schema::number())
                .required_property("kind", Schema::builder().constant("rect")),
        ),
    ])
    .build()
    .unwrap();

    assert_eq!(by_macro.to_json(), by_hand.to_json());
}

/// Whatever the macros write has to survive the meta-schema, since that is the
/// check `build()` runs on it.
#[test]
fn macro_output_is_valid_2020_12() {
    let schema = defs! {
        object! {
            required "shape": Schema::reference("#/$defs/Shape"),
            "extras": object! {},
        }
        .dialect()
        .id("https://example.com/s");

        "Shape": one_of_tagged! {
            "kind";
            "circle" => object! { required "radius": Schema::number() },
            "rect" => object! { required "width": Schema::number() },
        },
    }
    .build();

    assert!(schema.is_ok(), "{}", schema.unwrap_err());
}

/// A schema written with macros is an ordinary one: it exports, resolves, and
/// diffs like any other.
#[test]
fn a_macro_written_schema_works_with_the_rest_of_the_crate() {
    use ash_jsonschema::diff::{Role, diff};
    use ash_jsonschema::Registry;

    let v1 = object! {
        required "id": Schema::string(),
        "nickname": Schema::string(),
    }
    .id("https://example.com/user")
    .build()
    .unwrap();

    let v2 = object! {
        required "id": Schema::string(),
        required "email": Schema::string(),
    }
    .id("https://example.com/user")
    .build()
    .unwrap();

    // Diffing reads the same as it would for a hand-written schema.
    let report = diff(&v1, &v2);
    assert!(report.is_breaking_for(Role::Writer));
    assert!(report.is_breaking_for(Role::Reader));

    // And it indexes.
    let mut registry = Registry::new();
    registry.insert(&v2).unwrap();
    registry.check_references().unwrap();

    // And exports.
    let id = v2.export_at("/properties/id").unwrap();
    assert_eq!(id, Schema::string().build().unwrap());
}

//! Builds a schema, then checks some payloads against it.
//!
//! ```sh
//! cargo run --example validate
//! ```

use ash_jsonschema_builder::validator::Mode;
use ash_jsonschema_builder::{Format, Schema, object, validator};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The schema is fixed once the validator holds it: there is no way to
    // change what this checks against without building another one.
    let order = validator! {
        object! {
            required "id": Schema::string().format(Format::Uuid),
            required "total": object! {
                required "amount": Schema::integer().minimum(0),
                required "currency": Schema::string().pattern("^[A-Z]{3}$"),
            }.closed(),
            required "items": Schema::array()
                .items(object! {
                    required "sku": Schema::string().min_length(1),
                    required "quantity": Schema::integer().minimum(1),
                }.closed())
                .min_items(1),
            "note": Schema::string().max_length(280).nullable(),
        }
        .closed()
    }?;

    let good = serde_json::json!({
        "id": "8f14e45f-ceea-467a-9f8a-1b2c3d4e5f60",
        "total": {"amount": 2599, "currency": "EUR"},
        "items": [{"sku": "ASH-1", "quantity": 2}],
        "note": null,
    });
    println!("valid order: {}\n", order.validate(&good, Mode::All));

    // Wrong in several ways at once, so the two modes differ visibly.
    let bad = serde_json::json!({
        "id": "not-a-uuid",
        "total": {"amount": -5, "currency": "eur", "vat": 0.19},
        "items": [],
        "extra": true,
    });

    println!("everything wrong with it:");
    let outcome = order.validate(&bad, Mode::All);
    println!("{outcome}\n");

    // The same instance, stopping at the first problem — cheaper when the only
    // question is whether it passed.
    let first = order.validate(&bad, Mode::FailFast);
    println!("fail-fast reports {} of them\n", first.errors.len());

    // Errors carry where in the data they are, so they can be routed back to
    // whoever owns that part of the payload.
    println!("just the `total` field:");
    for error in outcome.at("/total") {
        println!("  {} — {}", error.instance_location, error.message());
    }

    // `format` is an annotation in 2020-12, not an assertion, so the malformed
    // `id` above is deliberately *not* reported. Everything else is.
    println!("\nnote: `format` is an annotation in 2020-12 and is not asserted,");
    println!("so `\"id\": \"not-a-uuid\"` passes. The other failures are real.");

    Ok(())
}

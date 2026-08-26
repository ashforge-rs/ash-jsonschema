//! Builds a small order document and prints it.
//!
//! ```sh
//! cargo run --example order
//! ```

use ash_jsonschema::{Format, Schema, SchemaError};

fn main() -> Result<(), SchemaError> {
    let money = Schema::object()
        .required_property("amount", Schema::integer().minimum(0))
        .required_property("currency", Schema::string().length(3).pattern("^[A-Z]{3}$"))
        .additional_properties(false);

    let schema = Schema::object()
        .dialect()
        .id("https://example.com/schemas/order")
        .title("Order")
        .description("A customer order.")
        .def("Money", money)
        .required_property("id", Schema::string().format(Format::Uuid))
        .required_property("placed_at", Schema::string().format(Format::DateTime))
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
        .property(
            "channel",
            Schema::string()
                .enumeration(["web", "mobile", "phone"])
                .default("web"),
        )
        .additional_properties(false)
        // Checked against the embedded 2020-12 meta-schema, root and every
        // subschema under it, before it is handed back.
        .build()?;

    println!("{schema}");
    Ok(())
}

//! Splits a document across two schemas and resolves the reference between
//! them, then shows what a typo in a `$ref` looks like.
//!
//! ```sh
//! cargo run --example registry
//! ```

use ash_jsonschema::{Format, Registry, Schema};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Two documents, each with its own `$id`, one referring to the other.
    let address = Schema::object()
        .dialect()
        .id("https://example.com/schemas/address")
        .title("Address")
        .required_property("street", Schema::string().min_length(1))
        .required_property("city", Schema::string().min_length(1))
        .required_property("country", Schema::string().length(2))
        .closed()
        .build()?;

    let user = Schema::object()
        .dialect()
        .id("https://example.com/schemas/user")
        .title("User")
        .required_property("id", Schema::string().format(Format::Uuid))
        // Relative: resolved against this document's own `$id`.
        .required_property("address", Schema::reference("address"))
        // Local: into this document's `$defs`.
        .property("backup_address", Schema::reference("#/$defs/MaybeAddress"))
        .def(
            "MaybeAddress",
            Schema::builder().any_of([
                Schema::reference("address").build_unchecked(),
                Schema::null().build_unchecked(),
            ]),
        )
        .closed()
        .build()?;

    let mut registry = Registry::new();
    registry.insert(&address)?;
    registry.insert(&user)?;

    println!("indexed {} resources:", registry.len());
    for uri in registry.resources() {
        println!("  {uri}");
    }

    println!("\nreferences found:");
    for reference in registry.references() {
        println!("  {} → {}", reference.location, reference.absolute());
    }

    // Every one of them lands somewhere.
    registry.check_references()?;
    println!("\nall references resolve.");

    // Following one hands back the schema it names.
    let target = registry
        .resolve("https://example.com/schemas/user#/$defs/MaybeAddress")
        .expect("the definition is indexed");
    println!("\n#/$defs/MaybeAddress resolves to:\n{target:#}");

    // A `$ref` that names nothing is a legal document, so it builds — and the
    // registry is what notices.
    let typo = Schema::object()
        .id("https://example.com/schemas/typo")
        .property("address", Schema::reference("#/$defs/Adress"))
        .build()?;

    let mut broken = Registry::new();
    broken.insert(&typo)?;
    match broken.check_references() {
        Ok(()) => unreachable!("the reference is dangling"),
        Err(error) => println!("\nthe typo is caught:\n{error}"),
    }

    Ok(())
}

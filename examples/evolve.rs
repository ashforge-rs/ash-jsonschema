//! Two versions of an API schema: what changed, who it breaks, and how to ship
//! the result as one file.
//!
//! ```sh
//! cargo run --example evolve
//! ```

use ash_jsonschema::bundle::bundle;
use ash_jsonschema::diff::{Role, diff};
use ash_jsonschema::{Format, Registry, Schema};

/// The `user` schema as it stood, and as it is proposed.
fn user(version: u8) -> Schema {
    let mut builder = Schema::object()
        .dialect()
        .id("https://example.com/schemas/user")
        .title("User")
        .required_property("id", Schema::string().format(Format::Uuid))
        .required_property("address", Schema::reference("address"));

    builder = match version {
        1 => builder
            .property("nickname", Schema::string())
            .property("age", Schema::integer().minimum(0)),
        _ => builder
            // A new required field: existing data does not carry it.
            .required_property("email", Schema::string().format(Format::Email))
            // `nickname` is gone: a reader that showed it no longer can.
            // A tighter bound: some previously valid ages are now refused.
            .property("age", Schema::integer().minimum(13)),
    };

    builder.build().expect("the schema is well formed")
}

fn address() -> Schema {
    Schema::object()
        .dialect()
        .id("https://example.com/schemas/address")
        .required_property("city", Schema::string().min_length(1))
        .required_property("country", Schema::string().length(2))
        .build()
        .expect("the schema is well formed")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (v1, v2) = (user(1), user(2));

    // --- what changed ----------------------------------------------------
    let report = diff(&v1, &v2);
    println!("{report}\n");

    // The two sides of the contract break differently, and a release gate
    // cares about whichever one it is guarding.
    for role in [Role::Writer, Role::Reader] {
        let breaking: Vec<_> = report.breaking_for(role).collect();
        match breaking.is_empty() {
            true => println!("{role}s: unaffected"),
            false => {
                println!("{role}s: {} breaking change(s)", breaking.len());
                for change in breaking {
                    println!("    {}", change.kind);
                }
            }
        }
    }

    // --- shipping it as one file ------------------------------------------
    let mut registry = Registry::new();
    registry.insert(&address())?;
    registry.insert(&v2)?;

    // Worth knowing before bundling: an unresolved reference would make the
    // bundle quietly incomplete, so `bundle` refuses rather than guessing.
    registry.check_references()?;

    let bundled = bundle(&registry, "https://example.com/schemas/user")?;

    // The bundle stands alone: nothing else need be present for it to resolve.
    let mut shipped = Registry::new();
    shipped.insert(&bundled)?;
    shipped.check_references()?;

    println!(
        "\nbundled {} resource(s) into one document:",
        shipped.len()
    );
    for uri in shipped.resources() {
        println!("    {uri}");
    }
    println!("\n{bundled}");

    Ok(())
}

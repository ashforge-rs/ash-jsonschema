//! The four output formats the specification defines, side by side.
//!
//! ```sh
//! cargo run --example output
//! ```

use ash_jsonschema::validator::{Mode, OutputFormat};
use ash_jsonschema::{Schema, Validator};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // An `anyOf` is the case that makes the difference between the formats
    // obvious: the top-level message can only say "nothing matched", and the
    // useful part is *why each alternative did not*.
    let schema = Schema::parse(
        r#"{
            "title": "Payment",
            "anyOf": [
                {"type": "string", "pattern": "^card_", "title": "a card token"},
                {"type": "object", "required": ["iban"], "title": "a bank transfer"}
            ]
        }"#,
    )?;

    let validator = Validator::new(&schema)?;
    let instance = serde_json::json!({"account": "12345"});
    let outcome = validator.validate(&instance, Mode::All);

    println!("{outcome}\n");

    for format in [
        OutputFormat::Flag,
        OutputFormat::Basic,
        OutputFormat::Detailed,
    ] {
        let rendered = serde_json::to_string_pretty(&outcome.output(format))?;
        println!("--- {format} ---\n{rendered}\n");
    }

    // Annotations are the other half of what the output formats carry, and
    // they are collected only when asked for — a plain `validate` never pays
    // for them.
    let valid = serde_json::json!("card_9f2");
    let annotated = validator.annotate(&valid, Mode::All);
    println!("--- annotations on a valid instance ---");
    for unit in &annotated.annotations {
        println!(
            "  {:20} {} = {}",
            match unit.instance_location.is_empty() {
                true => "(root)",
                false => &unit.instance_location,
            },
            unit.keyword(),
            unit.annotation.clone().unwrap_or_default(),
        );
    }

    // The branch that did not match contributes nothing: an annotation from a
    // rejected schema describes an instance that does not exist.
    println!("\n(only the matching alternative annotates — `a bank transfer` is absent)",);

    Ok(())
}

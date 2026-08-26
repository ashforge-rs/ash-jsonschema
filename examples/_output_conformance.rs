//! Checks our output against the suite's own output tests.
//!
//! Each expected output in the suite is itself a JSON Schema that our produced
//! output must satisfy — so this validates our validator with our validator.

use ash_jsonschema_builder::validator::{Mode, OutputFormat};
use ash_jsonschema_builder::{Registry, Schema, Validator};
use serde_json::Value;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args().nth(1).expect("suite dir");
    let content = format!("{dir}/output-tests/draft2020-12/content");
    let output_schema_path = format!("{dir}/output-tests/draft2020-12/output-schema.json");

    let output_schema: Value =
        serde_json::from_str(&std::fs::read_to_string(&output_schema_path)?)?;

    let mut pass = 0;
    let mut fail = 0;

    let mut files: Vec<_> = std::fs::read_dir(&content)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();

    for file in files {
        let groups: Value = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
        let name = file.file_name().unwrap().to_string_lossy().to_string();

        for group in groups.as_array().unwrap() {
            let schema_value = &group["schema"];
            let schema = Schema::from_value(schema_value.clone())?;
            let validator = match Validator::new(&schema) {
                Ok(v) => v,
                Err(e) => {
                    println!("[{name}] compile failed: {e}");
                    fail += 1;
                    continue;
                }
            };

            for test in group["tests"].as_array().unwrap() {
                let data = &test["data"];
                let outcome = validator.annotate(data, Mode::All);

                for (format_name, expected) in test["output"].as_object().unwrap() {
                    let format = match format_name.as_str() {
                        "flag" => OutputFormat::Flag,
                        "basic" => OutputFormat::Basic,
                        "detailed" => OutputFormat::Detailed,
                        "verbose" => OutputFormat::Verbose,
                        other => panic!("unknown format {other}"),
                    };
                    let produced = outcome.output(format);

                    // The expected value is a schema our output must satisfy.
                    // It `$ref`s the official output schema, so register that.
                    let mut registry = Registry::new();
                    // The output schema carries its own `$id`.
                    registry.insert(&Schema::from_value(output_schema.clone())?)?;

                    let expect_schema = Schema::from_value(expected.clone())?;
                    let checker = match Validator::with_registry(&expect_schema, &registry) {
                        Ok(v) => v,
                        Err(e) => {
                            println!("[{name}] expectation compile failed: {e}");
                            fail += 1;
                            continue;
                        }
                    };

                    let result = checker.validate(&produced, Mode::All);
                    if result.is_valid() {
                        pass += 1;
                    } else {
                        fail += 1;
                        println!(
                            "[{name}] {} / {} ({format_name})\n  produced: {}\n  {}",
                            group["description"].as_str().unwrap_or(""),
                            test["description"].as_str().unwrap_or(""),
                            serde_json::to_string(&produced)?,
                            result,
                        );
                    }
                }
            }
        }
    }

    println!("\noutput conformance: {pass} pass, {fail} fail");
    Ok(())
}

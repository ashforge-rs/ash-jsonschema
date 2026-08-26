//! Runs the official JSON-Schema-Test-Suite.
use ash_jsonschema_builder::validator::Mode;
use ash_jsonschema_builder::{Registry, Schema, Validator};
use serde_json::Value;

fn load_remotes(dir: &str) -> Registry {
    let mut registry = Registry::new();
    fn walk(path: &std::path::Path, base: &std::path::Path, registry: &mut Registry) {
        let Ok(entries) = std::fs::read_dir(path) else { return };
        for entry in entries.filter_map(Result::ok) {
            let p = entry.path();
            if p.is_dir() { walk(&p, base, registry); continue; }
            if p.extension().is_some_and(|e| e == "json")
                && let Ok(text) = std::fs::read_to_string(&p)
                && let Ok(value) = serde_json::from_str::<Value>(&text)
            {
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                let uri = format!("http://localhost:1234/{rel}");
                let mut v = value;
                if v.is_object() && v.get("$id").is_none() {
                    v.as_object_mut().unwrap().insert("$id".into(), Value::String(uri.clone()));
                }
                if let Ok(schema) = Schema::from_value(v) { let _ = registry.insert(&schema); }
            }
        }
    }
    let base = std::path::Path::new(dir);
    walk(base, base, &mut registry);
    registry
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args().nth(1).expect("suite dir");
    let sub = std::env::args().nth(2).unwrap_or_else(|| "tests/draft2020-12".into());
    let registry = load_remotes(&format!("{dir}/remotes"));

    let (mut pass, mut fail) = (0u32, 0u32);
    let mut by_file: Vec<(String, u32, u32)> = Vec::new();

    let mut files: Vec<_> = std::fs::read_dir(format!("{dir}/{sub}"))?
        .filter_map(Result::ok).map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
    files.sort();

    for file in files {
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        let (mut fp, mut ff) = (0u32, 0u32);
        let groups: Value = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
        for group in groups.as_array().unwrap() {
            let Ok(schema) = Schema::from_value(group["schema"].clone()) else { ff += 1; continue };
            let validator = match Validator::with_registry(&schema, &registry) {
                Ok(v) => v, Err(_) => { ff += group["tests"].as_array().map_or(1, |t| t.len() as u32); continue }
            };
            for test in group["tests"].as_array().unwrap() {
                let want = test["valid"].as_bool().unwrap();
                let got = validator.validate(&test["data"], Mode::All).is_valid();
                if got == want { fp += 1 } else {
                    ff += 1;
                    if std::env::var("VERBOSE").is_ok() {
                        println!("  [{name}] {} / {}: want {want} got {got}",
                            group["description"].as_str().unwrap_or(""),
                            test["description"].as_str().unwrap_or(""));
                    }
                }
            }
        }
        pass += fp; fail += ff;
        if ff > 0 { by_file.push((name, fp, ff)); }
    }
    for (n, p, f) in &by_file { println!("{n}: {p} pass, {f} fail"); }
    let total = pass + fail;
    println!("\nTOTAL: {pass}/{total} ({:.1}%), {fail} fail", 100.0 * pass as f64 / total as f64);
    Ok(())
}

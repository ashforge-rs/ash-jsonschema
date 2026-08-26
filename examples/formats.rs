//! Asserting `format` — with the built-in checks, and with rules of your own.
//!
//! ```sh
//! cargo run --example formats
//! cargo run --example formats --features iana-tlds
//! ```

use ash_jsonschema_builder::validator::Mode;
use ash_jsonschema_builder::validator::tld::has_tld_shaped_suffix;
use ash_jsonschema_builder::{Schema, Validator, object};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let schema = object! {
        required "starts_on": Schema::string().format("date"),
        required "contact": Schema::string().format("email"),
        required "site": Schema::string().format("hostname"),
        required "runs_for": Schema::string().format("duration"),
        required "ticket": Schema::string().format("ticket-ref"),
    }
    .build()?;

    // Without asking, `format` annotates and asserts nothing — which is what
    // 2020-12 specifies by default.
    let lenient = Validator::new(&schema)?;
    let nonsense = serde_json::json!({
        "starts_on": "whenever",
        "contact": "not an address",
        "site": "not a host",
        "runs_for": "ages",
        "ticket": "???",
    });
    println!(
        "annotation by default: {}\n",
        lenient.validate(&nonsense, Mode::All)
    );

    // Asked to assert, plus one name the crate has never heard of.
    let strict = Validator::builder(&schema)
        .assert_formats()
        // A rule of your own. The schema stays ordinary JSON — another
        // validator simply annotates `ticket-ref` where this one asserts it.
        .format("ticket-ref", |value| {
            value
                .strip_prefix("ASH-")
                .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
        })
        // Overriding a built-in: a hostname must also carry a real TLD.
        .format("hostname", has_tld_shaped_suffix)
        .build()?;

    println!("asserted:\n{}\n", strict.validate(&nonsense, Mode::All));

    let good = serde_json::json!({
        "starts_on": "2024-02-29",
        "contact": "team@example.com",
        "site": "docs.example.com",
        "runs_for": "P1Y2M",
        "ticket": "ASH-1042",
    });
    println!("a well-formed record: {}\n", strict.validate(&good, Mode::All));

    // The calendar is checked, not just the shape.
    println!("dates are real dates:");
    for candidate in ["2024-02-29", "2023-02-29", "2024-04-31", "2024-1-5"] {
        let instance = serde_json::json!({
            "starts_on": candidate,
            "contact": "team@example.com",
            "site": "docs.example.com",
            "runs_for": "P1D",
            "ticket": "ASH-1",
        });
        let verdict = match strict.validate(&instance, Mode::FailFast).is_valid() {
            true => "valid",
            false => "rejected",
        };
        println!("  {candidate:12} {verdict}");
    }

    #[cfg(feature = "iana-tlds")]
    {
        use ash_jsonschema_builder::validator::tld::{has_registered_tld, registered_tlds};

        println!(
            "\nwith `iana-tlds`: {} delegated TLDs are known",
            registered_tlds().len(),
        );
        for host in ["example.com", "example.museum", "example.zzz"] {
            // `.zzz` is shaped like a TLD and is not one, which is the whole
            // difference between the two checks.
            println!(
                "  {host:18} shaped: {:<5}  registered: {}",
                has_tld_shaped_suffix(host),
                has_registered_tld(host),
            );
        }
    }
    #[cfg(not(feature = "iana-tlds"))]
    println!("\n(build with `--features iana-tlds` to check TLDs against the IANA list)");

    Ok(())
}

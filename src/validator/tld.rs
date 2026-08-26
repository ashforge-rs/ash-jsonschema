//! Top-level domains: the structural rules, and the IANA list.
//!
//! There are two questions a hostname's suffix can be asked, and they are not
//! the same one:
//!
//! - **Is it shaped like a TLD?** No digits, no leading digit, at least two
//!   characters, and — for an internationalised one — a well-formed `xn--`
//!   prefix. This is a rule about form, it never goes out of date, and it is
//!   what [`is_tld_shaped`] answers.
//! - **Is it a TLD that exists?** A membership test against the IANA root
//!   zone, which changes as registries come and go. That is
//!   `is_registered_tld`, behind the **`iana-tlds`** feature.
//!
//! The distinction matters because a vendored list is a snapshot. A build from
//! last year rejects a domain delegated this year, and nothing in the schema
//! says so. Structure is therefore the default, and the list is opt-in with
//! that caveat visible.
//!
//! ```
//! # use ash_jsonschema_builder::{Schema, Validator};
//! let schema = Schema::string().format("hostname").build()?;
//!
//! // Registered as a `format`, so the schema stays ordinary JSON.
//! let validator = Validator::builder(&schema)
//!     .format("hostname", |value| {
//!         ash_jsonschema_builder::validator::tld::has_tld_shaped_suffix(value)
//!     })
//!     .build()?;
//!
//! assert!(validator.is_valid(&serde_json::json!("example.com")));
//! assert!(!validator.is_valid(&serde_json::json!("example.123")));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

/// Whether a label is shaped like a top-level domain.
///
/// The rules are the ones that hold regardless of which TLDs happen to exist:
/// at least two characters, ASCII letters only — or a well-formed `xn--`
/// punycode label — and no digits. The IANA root zone has never delegated a
/// TLD that breaks them.
///
/// ```
/// # use ash_jsonschema_builder::validator::tld::is_tld_shaped;
/// assert!(is_tld_shaped("com"));
/// assert!(is_tld_shaped("museum"));
/// assert!(is_tld_shaped("xn--p1ai"));      // рф
///
/// assert!(!is_tld_shaped("c"));            // too short
/// assert!(!is_tld_shaped("123"));          // digits
/// assert!(!is_tld_shaped("co2"));
/// assert!(!is_tld_shaped("xn--"));         // nothing encoded
/// ```
pub fn is_tld_shaped(label: &str) -> bool {
    let label = label.to_ascii_lowercase();

    // An internationalised TLD is punycode: `xn--` and a non-empty body of
    // letters, digits, and hyphens.
    if let Some(encoded) = label.strip_prefix("xn--") {
        return !encoded.is_empty()
            && encoded.len() <= 59
            && encoded
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-');
    }

    label.len() >= 2
        && label.len() <= 63
        && label.chars().all(|c| c.is_ascii_alphabetic())
}

/// Whether a hostname ends in a label shaped like a TLD.
///
/// A bare name with no dot has no TLD to check and is rejected: `localhost` is
/// a valid hostname but not a public domain, and this asks the latter.
///
/// ```
/// # use ash_jsonschema_builder::validator::tld::has_tld_shaped_suffix;
/// assert!(has_tld_shaped_suffix("example.com"));
/// assert!(has_tld_shaped_suffix("a.b.example.co.uk"));
/// assert!(has_tld_shaped_suffix("example.com."));   // fully qualified
///
/// assert!(!has_tld_shaped_suffix("localhost"));     // no TLD at all
/// assert!(!has_tld_shaped_suffix("example.123"));
/// assert!(!has_tld_shaped_suffix("192.168.0.1"));
/// ```
pub fn has_tld_shaped_suffix(hostname: &str) -> bool {
    let hostname = hostname.strip_suffix('.').unwrap_or(hostname);
    match hostname.rsplit_once('.') {
        Some((rest, tld)) => !rest.is_empty() && is_tld_shaped(tld),
        None => false,
    }
}

#[cfg(feature = "iana-tlds")]
mod iana {
    /// The vendored IANA root zone, one lowercase TLD per line.
    const LIST: &str = include_str!("../../resources/iana/tlds.txt");

    /// The list, parsed once and sorted for binary search.
    pub(super) fn tlds() -> &'static [&'static str] {
        use std::sync::OnceLock;
        static TLDS: OnceLock<Vec<&'static str>> = OnceLock::new();

        TLDS.get_or_init(|| {
            // The file is committed already sorted, which `is_sorted` below
            // holds it to, so no sorting is needed at load.
            LIST.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .collect()
        })
    }
}

/// Every top-level domain IANA had delegated when this crate was built.
///
/// *Requires the `iana-tlds` feature.*
///
/// ```
/// # use ash_jsonschema_builder::validator::tld::registered_tlds;
/// assert!(registered_tlds().contains(&"com"));
/// assert!(registered_tlds().len() > 1_000);
/// ```
#[cfg(feature = "iana-tlds")]
pub fn registered_tlds() -> &'static [&'static str] {
    iana::tlds()
}

/// Whether a label is a top-level domain IANA had delegated when this crate was
/// built.
///
/// *Requires the `iana-tlds` feature.*
///
/// The comparison is case-insensitive. **The list is a snapshot**: a TLD
/// delegated after this crate was published is not in it, and one withdrawn
/// since still is. Where that matters, register your own handler with a list
/// you refresh — or use [`is_tld_shaped`], which never goes stale.
///
/// ```
/// # use ash_jsonschema_builder::validator::tld::is_registered_tld;
/// assert!(is_registered_tld("com"));
/// assert!(is_registered_tld("COM"));
/// assert!(is_registered_tld("museum"));
///
/// assert!(!is_registered_tld("invalid-tld"));
/// assert!(!is_registered_tld("internal"));
/// ```
#[cfg(feature = "iana-tlds")]
pub fn is_registered_tld(label: &str) -> bool {
    let label = label.to_ascii_lowercase();
    iana::tlds().binary_search(&label.as_str()).is_ok()
}

/// Whether a hostname ends in a registered top-level domain.
///
/// *Requires the `iana-tlds` feature.* See [`is_registered_tld`] for the
/// caveat about the list being a snapshot.
///
/// ```
/// # use ash_jsonschema_builder::validator::tld::has_registered_tld;
/// assert!(has_registered_tld("example.com"));
/// assert!(has_registered_tld("a.b.example.co.uk"));
///
/// assert!(!has_registered_tld("example.invalid-tld"));
/// assert!(!has_registered_tld("localhost"));
/// ```
#[cfg(feature = "iana-tlds")]
pub fn has_registered_tld(hostname: &str) -> bool {
    let hostname = hostname.strip_suffix('.').unwrap_or(hostname);
    match hostname.rsplit_once('.') {
        Some((rest, tld)) => !rest.is_empty() && is_registered_tld(tld),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tld_must_be_letters_and_at_least_two_of_them() {
        assert!(is_tld_shaped("com"));
        assert!(is_tld_shaped("io"));
        assert!(is_tld_shaped("museum"));
        assert!(is_tld_shaped("COM")); // case-insensitive

        assert!(!is_tld_shaped("c"));
        assert!(!is_tld_shaped(""));
        assert!(!is_tld_shaped("123"));
        assert!(!is_tld_shaped("co2"));
        assert!(!is_tld_shaped("co-op")); // a hyphen needs the `xn--` form
    }

    #[test]
    fn an_internationalised_tld_is_punycode() {
        assert!(is_tld_shaped("xn--p1ai")); // рф
        assert!(is_tld_shaped("xn--fiqs8s")); // 中国

        assert!(!is_tld_shaped("xn--")); // nothing encoded
        assert!(!is_tld_shaped("xn--\u{4e2d}")); // not ASCII
    }

    #[test]
    fn a_suffix_needs_something_in_front_of_it() {
        assert!(has_tld_shaped_suffix("example.com"));
        assert!(has_tld_shaped_suffix("a.b.c.example.com"));
        assert!(has_tld_shaped_suffix("example.com.")); // fully qualified

        assert!(!has_tld_shaped_suffix("localhost")); // no dot
        assert!(!has_tld_shaped_suffix(".com")); // nothing before it
        assert!(!has_tld_shaped_suffix("example.123"));
        assert!(!has_tld_shaped_suffix("192.168.0.1"));
        assert!(!has_tld_shaped_suffix(""));
    }

    #[cfg(feature = "iana-tlds")]
    #[test]
    fn the_vendored_list_is_sorted_and_lowercase() {
        let tlds = registered_tlds();

        assert!(tlds.len() > 1_000, "the list looks truncated");
        assert!(
            tlds.is_sorted(),
            "the list must stay sorted — `is_registered_tld` binary-searches it",
        );
        for tld in tlds {
            assert_eq!(
                *tld,
                tld.to_ascii_lowercase(),
                "`{tld}` is not lowercase, so a lookup would miss it",
            );
        }
    }

    #[cfg(feature = "iana-tlds")]
    #[test]
    fn membership_is_case_insensitive() {
        assert!(is_registered_tld("com"));
        assert!(is_registered_tld("COM"));
        assert!(is_registered_tld("Com"));
        assert!(is_registered_tld("xn--p1ai"));

        assert!(!is_registered_tld("invalid-tld"));
        assert!(!is_registered_tld(""));
    }

    #[cfg(feature = "iana-tlds")]
    #[test]
    fn a_registered_tld_is_also_a_well_shaped_one() {
        // Every delegated TLD satisfies the structural rule, which is what
        // makes `is_tld_shaped` a safe default when the list is not compiled in.
        for tld in registered_tlds() {
            assert!(
                is_tld_shaped(tld),
                "`{tld}` is delegated but fails the structural rule",
            );
        }
    }

    #[cfg(feature = "iana-tlds")]
    #[test]
    fn a_hostname_is_checked_against_its_last_label() {
        assert!(has_registered_tld("example.com"));
        assert!(has_registered_tld("a.b.example.co.uk"));
        assert!(has_registered_tld("EXAMPLE.COM"));

        assert!(!has_registered_tld("example.invalid-tld"));
        assert!(!has_registered_tld("localhost"));
    }
}

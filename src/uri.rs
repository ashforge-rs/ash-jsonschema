//! Resolving one URI reference against another, RFC 3986 §5.
//!
//! JSON Schema addresses everything by URI: `$id` establishes a base, `$ref`
//! names a target relative to it, and the fragment that comes back is either a
//! JSON Pointer or a plain-name anchor. This module does that resolution and
//! nothing else — it neither knows nor cares what a schema is.
//!
//! The implementation is deliberately partial. It handles the reference forms
//! that appear in schema documents — absolute URIs, absolute paths, relative
//! paths, and bare fragments — and treats a scheme as anything before `://`.
//! Userinfo, ports, and query strings pass through untouched because they are
//! never significant in a `$ref`, and no attempt is made to normalise case or
//! percent-encoding, so two URIs that differ only in escaping are two different
//! resources here.

/// Splits a URI reference into the resource it names and its fragment.
///
/// The resource is `reference` resolved against `base`; the fragment is
/// returned separately and **unescaped is not attempted** — a pointer fragment
/// comes back with its leading `/`, and a plain-name anchor comes back bare.
///
/// ```
/// # use ash_jsonschema::uri::resolve;
/// let base = "https://example.com/schemas/user";
///
/// // A bare fragment keeps the base resource.
/// assert_eq!(
///     resolve(base, "#/$defs/Address"),
///     ("https://example.com/schemas/user".to_owned(), "/$defs/Address".to_owned()),
/// );
///
/// // A relative path replaces the base's last segment.
/// assert_eq!(
///     resolve(base, "address"),
///     ("https://example.com/schemas/address".to_owned(), String::new()),
/// );
///
/// // An absolute URI replaces the base outright.
/// assert_eq!(
///     resolve(base, "https://other.example/x#a"),
///     ("https://other.example/x".to_owned(), "a".to_owned()),
/// );
/// ```
pub fn resolve(base: &str, reference: &str) -> (String, String) {
    let (target, fragment) = match reference.split_once('#') {
        Some((target, fragment)) => (target, fragment),
        None => (reference, ""),
    };
    let base_resource = base.split_once('#').map_or(base, |(resource, _)| resource);

    let resource = if target.is_empty() {
        base_resource.to_owned()
    } else if target.contains("://") {
        target.to_owned()
    } else if let Some(rest) = target.strip_prefix('/') {
        // An absolute path replaces the base's whole path.
        match base_resource.split_once("://") {
            Some((scheme, rest_of_base)) => {
                let authority = rest_of_base.split('/').next().unwrap_or("");
                format!("{scheme}://{authority}/{rest}")
            }
            None => target.to_owned(),
        }
    } else {
        // A relative path replaces the base's last segment.
        let directory = match base_resource.rfind('/') {
            Some(index) => &base_resource[..=index],
            None => "",
        };
        remove_dot_segments(&format!("{directory}{target}"))
    };

    (resource, fragment.to_owned())
}

/// Whether a fragment is a JSON Pointer rather than a plain-name anchor.
///
/// The two are told apart by their first character: RFC 6901 requires a
/// pointer to be empty or to start with `/`, and the 2020-12 anchor grammar
/// forbids a leading `/`, so the test is unambiguous.
///
/// ```
/// # use ash_jsonschema::uri::is_pointer_fragment;
/// assert!(is_pointer_fragment(""));            // the whole resource
/// assert!(is_pointer_fragment("/$defs/A"));
/// assert!(!is_pointer_fragment("my-anchor"));
/// ```
pub fn is_pointer_fragment(fragment: &str) -> bool {
    fragment.is_empty() || fragment.starts_with('/')
}

/// Collapses `.` and `..` segments in a path, RFC 3986 §5.2.4.
pub fn remove_dot_segments(uri: &str) -> String {
    let (prefix, path) = match uri.split_once("://") {
        Some((scheme, rest)) => match rest.split_once('/') {
            Some((authority, path)) => (format!("{scheme}://{authority}/"), path),
            None => return uri.to_owned(),
        },
        None => (String::new(), uri),
    };

    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    format!("{prefix}{}", segments.join("/"))
}

/// Decodes the percent-escapes in a URI fragment.
///
/// A fragment is part of a URI, so a JSON Pointer written in one is
/// percent-encoded: a property named `percent%field` is referenced as
/// `#/$defs/percent%25field`, and a `"` in a name as `%22`. The pointer has to
/// be decoded before it is resolved, or it names nothing.
///
/// Invalid escapes are left as they are rather than being an error — a fragment
/// this crate cannot decode is one it will simply fail to resolve, which is
/// already the reported outcome.
///
/// ```
/// # use ash_jsonschema::uri::decode_fragment;
/// assert_eq!(decode_fragment("/$defs/percent%25field"), "/$defs/percent%field");
/// assert_eq!(decode_fragment("/$defs/foo%22bar"), r#"/$defs/foo"bar"#);
/// assert_eq!(decode_fragment("/$defs/plain"), "/$defs/plain");
/// ```
pub fn decode_fragment(fragment: &str) -> String {
    if !fragment.contains('%') {
        return fragment.to_owned();
    }

    let bytes = fragment.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (
                (bytes[index + 1] as char).to_digit(16),
                (bytes[index + 2] as char).to_digit(16),
            )
        {
            out.push((high * 16 + low) as u8);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }

    String::from_utf8(out).unwrap_or_else(|_| fragment.to_owned())
}

/// Strips any fragment from a URI, leaving the resource it identifies.
///
/// ```
/// # use ash_jsonschema::uri::resource_of;
/// assert_eq!(resource_of("https://example.com/a#/b"), "https://example.com/a");
/// assert_eq!(resource_of("https://example.com/a"), "https://example.com/a");
/// ```
pub fn resource_of(uri: &str) -> &str {
    uri.split_once('#').map_or(uri, |(resource, _)| resource)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_references_resolve_against_the_base() {
        let base = "https://json-schema.org/draft/2020-12/schema";
        assert_eq!(
            resolve(base, "meta/core"),
            (
                "https://json-schema.org/draft/2020-12/meta/core".to_owned(),
                String::new()
            ),
        );
        assert_eq!(
            resolve(base, "#/$defs/anchorString"),
            (base.to_owned(), "/$defs/anchorString".to_owned()),
        );
        assert_eq!(resolve(base, "#meta"), (base.to_owned(), "meta".to_owned()));
        assert_eq!(
            resolve(base, "https://example.com/other#x"),
            ("https://example.com/other".to_owned(), "x".to_owned()),
        );
        assert_eq!(
            resolve(base, "/absolute"),
            ("https://json-schema.org/absolute".to_owned(), String::new()),
        );
    }

    #[test]
    fn an_empty_reference_keeps_the_base_resource() {
        let base = "https://example.com/a/b";
        assert_eq!(resolve(base, ""), (base.to_owned(), String::new()));
        assert_eq!(resolve(base, "#"), (base.to_owned(), String::new()));
    }

    #[test]
    fn a_fragment_on_the_base_is_dropped_before_resolving() {
        assert_eq!(
            resolve("https://example.com/a#/ignored", "b"),
            ("https://example.com/b".to_owned(), String::new()),
        );
    }

    #[test]
    fn dot_segments_collapse() {
        assert_eq!(
            remove_dot_segments("https://a.example/b/./c/../d"),
            "https://a.example/b/d",
        );
        assert_eq!(
            remove_dot_segments("https://a.example/b"),
            "https://a.example/b"
        );
        // A relative path with no authority is collapsed as-is.
        assert_eq!(remove_dot_segments("a/b/../c"), "a/c");
    }

    #[test]
    fn pointer_and_anchor_fragments_are_told_apart() {
        assert!(is_pointer_fragment(""));
        assert!(is_pointer_fragment("/"));
        assert!(is_pointer_fragment("/$defs/A"));
        assert!(!is_pointer_fragment("anchor"));
        assert!(!is_pointer_fragment("a.b-c_d"));
    }

    #[test]
    fn percent_escapes_in_a_fragment_are_decoded() {
        assert_eq!(decode_fragment("/$defs/percent%25field"), "/$defs/percent%field");
        assert_eq!(decode_fragment("/$defs/foo%22bar"), "/$defs/foo\"bar");
        assert_eq!(decode_fragment("/a%2Fb"), "/a/b");
        // Nothing to do, and nothing broken by trying.
        assert_eq!(decode_fragment("/$defs/plain"), "/$defs/plain");
        assert_eq!(decode_fragment(""), "");
        // A malformed escape is left alone rather than erroring.
        assert_eq!(decode_fragment("/a%zz"), "/a%zz");
        assert_eq!(decode_fragment("/a%"), "/a%");
    }

    #[test]
    fn a_resource_is_the_uri_without_its_fragment() {
        assert_eq!(resource_of("https://example.com/a#/b"), "https://example.com/a");
        assert_eq!(resource_of("https://example.com/a#"), "https://example.com/a");
        assert_eq!(resource_of("https://example.com/a"), "https://example.com/a");
    }
}

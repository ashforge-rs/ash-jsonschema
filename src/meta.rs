//! The JSON Schema 2020-12 meta-schema, embedded in the binary and parsed once.
//!
//! The eight documents that make up the dialect — the core-and-validation
//! meta-schema plus one document per vocabulary — are vendored under
//! `resources/2020-12/` and pulled in with [`include_str!`]. Nothing is fetched
//! at runtime: the specification the crate validates against is the copy it was
//! compiled with, so a build is reproducible and works offline.
//!
//! [`MetaSchema::current`] parses them on first use and caches the result for
//! the life of the process.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex_lite::Regex;
use serde_json::Value;

/// The retrieval URI and vendored text of one meta-schema document.
#[derive(Clone, Copy, Debug)]
pub struct Document {
    /// The absolute URI the document is published at, which is also its `$id`.
    pub uri: &'static str,
    /// The document's JSON text, exactly as published.
    pub text: &'static str,
}

/// The eight documents of the 2020-12 dialect, root first.
pub const DOCUMENTS: [Document; 8] = [
    Document {
        uri: "https://json-schema.org/draft/2020-12/schema",
        text: include_str!("../resources/2020-12/schema.json"),
    },
    Document {
        uri: "https://json-schema.org/draft/2020-12/meta/core",
        text: include_str!("../resources/2020-12/meta/core.json"),
    },
    Document {
        uri: "https://json-schema.org/draft/2020-12/meta/applicator",
        text: include_str!("../resources/2020-12/meta/applicator.json"),
    },
    Document {
        uri: "https://json-schema.org/draft/2020-12/meta/unevaluated",
        text: include_str!("../resources/2020-12/meta/unevaluated.json"),
    },
    Document {
        uri: "https://json-schema.org/draft/2020-12/meta/validation",
        text: include_str!("../resources/2020-12/meta/validation.json"),
    },
    Document {
        uri: "https://json-schema.org/draft/2020-12/meta/meta-data",
        text: include_str!("../resources/2020-12/meta/meta-data.json"),
    },
    Document {
        uri: "https://json-schema.org/draft/2020-12/meta/format-annotation",
        text: include_str!("../resources/2020-12/meta/format-annotation.json"),
    },
    Document {
        uri: "https://json-schema.org/draft/2020-12/meta/content",
        text: include_str!("../resources/2020-12/meta/content.json"),
    },
];

/// The parsed dialect, with its documents indexed for reference resolution.
#[derive(Debug)]
pub struct MetaSchema {
    documents: HashMap<&'static str, Value>,
    /// `(resource uri, anchor name)` → JSON Pointer within that resource.
    anchors: HashMap<(String, String), String>,
    /// The same, for `$dynamicAnchor`.
    dynamic_anchors: HashMap<(String, String), String>,
    /// Every `pattern` value and `patternProperties` key in the dialect,
    /// compiled once at load. The schema side is only ever the meta-schema, so
    /// this set is closed and no regex can fail to compile at validation time.
    regexes: HashMap<String, Regex>,
}

impl MetaSchema {
    /// The dialect, parsed on first call and reused thereafter.
    ///
    /// ```
    /// # use ash_jsonschema_builder::meta::MetaSchema;
    /// let meta = MetaSchema::current();
    /// assert!(meta.root().is_object());
    /// ```
    pub fn current() -> &'static MetaSchema {
        static META: OnceLock<MetaSchema> = OnceLock::new();
        META.get_or_init(MetaSchema::load)
    }

    /// Parses the embedded documents and indexes their anchors.
    ///
    /// # Panics
    ///
    /// If a vendored document is not valid JSON. That would mean the crate was
    /// built from a corrupted checkout, which no caller can recover from.
    fn load() -> Self {
        let mut documents = HashMap::with_capacity(DOCUMENTS.len());
        let mut anchors = HashMap::new();
        let mut dynamic_anchors = HashMap::new();
        let mut regexes = HashMap::new();

        for document in DOCUMENTS {
            let parsed: Value = serde_json::from_str(document.text).unwrap_or_else(|error| {
                panic!(
                    "the vendored meta-schema {} is not valid JSON: {error}",
                    document.uri
                )
            });
            index_anchors(
                &parsed,
                document.uri,
                "",
                &mut anchors,
                &mut dynamic_anchors,
            );
            index_regexes(&parsed, &mut regexes);
            documents.insert(document.uri, parsed);
        }

        Self {
            documents,
            anchors,
            dynamic_anchors,
            regexes,
        }
    }

    /// The root meta-schema — the one a `$schema` of [`DIALECT`] names.
    ///
    /// [`DIALECT`]: crate::DIALECT
    pub fn root(&self) -> &Value {
        self.document(crate::schema::DIALECT)
            .expect("the root document is always embedded")
    }

    /// One document by its absolute URI.
    pub fn document(&self, uri: &str) -> Option<&Value> {
        self.documents.get(uri)
    }

    /// The JSON Pointer that `$anchor: name` marks within `resource`.
    pub(crate) fn anchor(&self, resource: &str, name: &str) -> Option<&str> {
        self.anchors
            .get(&(resource.to_owned(), name.to_owned()))
            .map(String::as_str)
    }

    /// The JSON Pointer that `$dynamicAnchor: name` marks within `resource`.
    pub(crate) fn dynamic_anchor(&self, resource: &str, name: &str) -> Option<&str> {
        self.dynamic_anchors
            .get(&(resource.to_owned(), name.to_owned()))
            .map(String::as_str)
    }

    /// The compiled form of a regex that appears in the dialect.
    ///
    /// # Panics
    ///
    /// If `pattern` is not one the vendored documents contain — the evaluator
    /// only ever asks about patterns it read out of them.
    pub(crate) fn regex(&self, pattern: &str) -> &Regex {
        self.regexes
            .get(pattern)
            .unwrap_or_else(|| panic!("the meta-schema holds no compiled regex /{pattern}/"))
    }
}

/// Walks a document compiling every regex it asserts with.
///
/// A pattern that will not compile is a corrupted checkout, the same class of
/// problem as a document that will not parse, so this panics rather than
/// degrading to a validator that silently skips the rule.
fn index_regexes(node: &Value, regexes: &mut HashMap<String, Regex>) {
    let compile = |pattern: &str, regexes: &mut HashMap<String, Regex>| {
        if !regexes.contains_key(pattern) {
            let compiled = Regex::new(pattern).unwrap_or_else(|error| {
                panic!("the vendored meta-schema holds an uncompilable regex /{pattern}/: {error}")
            });
            regexes.insert(pattern.to_owned(), compiled);
        }
    };

    match node {
        Value::Object(object) => {
            if let Some(pattern) = object.get("pattern").and_then(Value::as_str) {
                compile(pattern, regexes);
            }
            if let Some(Value::Object(patterns)) = object.get("patternProperties") {
                for pattern in patterns.keys() {
                    compile(pattern, regexes);
                }
            }
            for child in object.values() {
                index_regexes(child, regexes);
            }
        }
        Value::Array(items) => {
            for item in items {
                index_regexes(item, regexes);
            }
        }
        _ => {}
    }
}

/// Walks a document recording every `$anchor` and `$dynamicAnchor` against the
/// resource it belongs to.
///
/// A nested `$id` starts a new resource, so anchors below it are keyed to that
/// URI and pointers restart from it.
fn index_anchors(
    node: &Value,
    resource: &str,
    pointer: &str,
    anchors: &mut HashMap<(String, String), String>,
    dynamic_anchors: &mut HashMap<(String, String), String>,
) {
    let Some(object) = node.as_object() else {
        return;
    };

    let (resource, pointer) = match object.get("$id").and_then(Value::as_str) {
        // The vendored documents only ever carry absolute `$id`s, and a nested
        // one would rebase everything under it.
        Some(id) if pointer.is_empty() || id.contains("://") => (id, ""),
        _ => (resource, pointer),
    };

    if let Some(name) = object.get("$anchor").and_then(Value::as_str) {
        anchors.insert((resource.to_owned(), name.to_owned()), pointer.to_owned());
    }
    if let Some(name) = object.get("$dynamicAnchor").and_then(Value::as_str) {
        dynamic_anchors.insert((resource.to_owned(), name.to_owned()), pointer.to_owned());
    }

    for (key, child) in object {
        let child_pointer = format!("{pointer}/{}", escape_pointer_token(key));
        match child {
            Value::Object(_) => {
                index_anchors(child, resource, &child_pointer, anchors, dynamic_anchors)
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    let item_pointer = format!("{child_pointer}/{index}");
                    index_anchors(item, resource, &item_pointer, anchors, dynamic_anchors);
                }
            }
            _ => {}
        }
    }
}

/// Escapes a JSON Pointer reference token (RFC 6901 §3): `~` becomes `~0` and
/// `/` becomes `~1`, so a property name can sit inside a pointer.
pub fn escape_pointer_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

/// Unescapes a JSON Pointer reference token (RFC 6901 §3).
fn unescape_pointer_token(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

/// Resolves a JSON Pointer against `document`, RFC 6901.
///
/// The empty pointer is the document itself. Returns `None` if any token names
/// something absent, or if a token indexes into a scalar.
///
/// ```
/// # use ash_jsonschema_builder::meta::resolve_pointer;
/// let document = serde_json::json!({"properties": {"id": {"type": "string"}}});
///
/// assert_eq!(
///     resolve_pointer(&document, "/properties/id/type"),
///     Some(&serde_json::json!("string")),
/// );
/// assert_eq!(resolve_pointer(&document, "/nope"), None);
/// ```
pub fn resolve_pointer<'a>(document: &'a Value, pointer: &str) -> Option<&'a Value> {
    if pointer.is_empty() {
        return Some(document);
    }
    let mut node = document;
    for token in pointer.trim_start_matches('/').split('/') {
        let token = unescape_pointer_token(token);
        node = match node {
            Value::Object(object) => object.get(&token)?,
            Value::Array(items) => items.get(token.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(node)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_document_parses_and_declares_its_own_uri_as_its_id() {
        let meta = MetaSchema::current();
        for document in DOCUMENTS {
            let parsed = meta.document(document.uri).expect("document is indexed");
            assert_eq!(
                parsed.get("$id").and_then(Value::as_str),
                Some(document.uri),
                "{} does not declare its own $id",
                document.uri,
            );
        }
    }

    #[test]
    fn the_root_pulls_in_every_vocabulary() {
        let meta = MetaSchema::current();
        let vocabularies = meta
            .root()
            .get("$vocabulary")
            .and_then(Value::as_object)
            .unwrap();
        assert_eq!(vocabularies.len(), 7);
        assert!(vocabularies.contains_key("https://json-schema.org/draft/2020-12/vocab/core"));
    }

    #[test]
    fn each_document_carries_the_meta_dynamic_anchor() {
        let meta = MetaSchema::current();
        for document in DOCUMENTS {
            assert_eq!(
                meta.dynamic_anchor(document.uri, "meta"),
                Some(""),
                "{} does not anchor `meta` at its root",
                document.uri,
            );
        }
    }

    #[test]
    fn the_dialects_regexes_are_compiled_up_front() {
        let meta = MetaSchema::current();
        // `$anchor` / `$dynamicAnchor` names.
        assert!(meta.regex("^[A-Za-z_][-A-Za-z0-9._]*$").is_match("meta"));
        assert!(
            !meta
                .regex("^[A-Za-z_][-A-Za-z0-9._]*$")
                .is_match("not an anchor")
        );
        // `$id` may carry at most an empty fragment.
        assert!(meta.regex("^[^#]*#?$").is_match("https://example.com/x"));
        assert!(!meta.regex("^[^#]*#?$").is_match("https://example.com/x#/a"));
    }

    #[test]
    #[should_panic(expected = "no compiled regex")]
    fn asking_for_a_regex_outside_the_dialect_is_a_bug() {
        let _ = MetaSchema::current().regex("^never$");
    }

    #[test]
    fn pointers_resolve_into_the_indexed_documents() {
        let meta = MetaSchema::current();
        let core = meta
            .document("https://json-schema.org/draft/2020-12/meta/core")
            .unwrap();
        let anchor_string = resolve_pointer(core, "/$defs/anchorString").unwrap();
        assert_eq!(
            anchor_string.get("type").and_then(Value::as_str),
            Some("string")
        );

        assert_eq!(resolve_pointer(core, ""), Some(core));
        assert_eq!(resolve_pointer(core, "/$defs/nope"), None);
    }

    #[test]
    fn pointer_tokens_round_trip_through_escaping() {
        assert_eq!(escape_pointer_token("a/b"), "a~1b");
        assert_eq!(escape_pointer_token("a~b"), "a~0b");
        assert_eq!(unescape_pointer_token("a~1b"), "a/b");
        assert_eq!(unescape_pointer_token("a~0b"), "a~b");
        // `~01` must survive as `~1`, not collapse to `/`.
        assert_eq!(unescape_pointer_token(&escape_pointer_token("~1")), "~1");
    }
}

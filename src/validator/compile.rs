//! Preparing a schema for validation, once, so validating is only evaluation.
//!
//! Everything that can fail about a *schema* is settled here: every `pattern`
//! is compiled, every anchor indexed, every resource located. A
//! [`Validator`](crate::Validator) that exists is one whose schema was
//! understood, so [`validate`](crate::Validator::validate) has no failure mode
//! of its own — it answers about the instance and nothing else.

use std::collections::HashMap;
use std::sync::Arc;

use regex_lite::Regex;
use serde_json::Value;

use crate::meta::resolve_pointer;
use crate::registry::{ANONYMOUS, Registry};
use crate::uri;
use crate::walk::walk_subschemas;

/// A schema that cannot be turned into a validator.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompileError {
    /// A `pattern` or `patternProperties` key is not a regex this crate can
    /// compile.
    ///
    /// JSON Schema specifies ECMA-262 regular expressions. This crate uses
    /// `regex-lite`, which covers the common grammar but not lookaround or
    /// backreferences — a pattern using those is reported here rather than
    /// silently not being enforced.
    BadPattern {
        /// A JSON Pointer to the keyword holding it.
        location: String,
        /// The pattern as written.
        pattern: String,
        /// Why it would not compile.
        reason: String,
    },
    /// The schema, or one it references, is not a well-formed document.
    NotASchema {
        /// A JSON Pointer to the offending place.
        location: String,
    },
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadPattern {
                location,
                pattern,
                reason,
            } => write!(f, "{location}: cannot compile /{pattern}/: {reason}"),
            Self::NotASchema { location } => {
                write!(f, "{location}: not a schema")
            }
        }
    }
}

impl std::error::Error for CompileError {}

/// A schema prepared for validation.
///
/// Holds the document, the regexes its patterns compile to, and the index that
/// resolves its references. Shared behind an [`Arc`] so cloning a
/// [`Validator`](crate::Validator) is cheap and thread-safe.
#[derive(Debug)]
pub(crate) struct Compiled {
    /// The root document.
    document: Arc<Value>,
    /// The base URI the root establishes.
    base: String,
    /// Every pattern in the schema, compiled once.
    regexes: HashMap<String, Regex>,
    /// Resources reachable by reference: URI → document.
    resources: HashMap<String, Arc<Value>>,
    /// `(resource, anchor)` → pointer within that resource.
    anchors: HashMap<(String, String), String>,
    /// The same for `$dynamicAnchor`.
    dynamic_anchors: HashMap<(String, String), String>,
    /// Resource URIs in insertion order, which `$dynamicRef` searches.
    order: Vec<String>,
}

impl Compiled {
    /// Prepares `document`, resolving references through `registry` when one is
    /// given.
    pub(crate) fn new(
        document: Arc<Value>,
        registry: Option<&Registry>,
    ) -> Result<Self, CompileError> {
        if !matches!(*document, Value::Object(_) | Value::Bool(_)) {
            return Err(CompileError::NotASchema {
                location: String::new(),
            });
        }

        let base = document
            .get("$id")
            .and_then(Value::as_str)
            .map(|id| uri::resource_of(id).to_owned())
            .unwrap_or_else(|| ANONYMOUS.to_owned());

        let mut compiled = Self {
            document: Arc::clone(&document),
            base: base.clone(),
            regexes: HashMap::new(),
            resources: HashMap::new(),
            anchors: HashMap::new(),
            dynamic_anchors: HashMap::new(),
            order: Vec::new(),
        };

        compiled.index(&base, Arc::clone(&document))?;

        // Anything the registry holds is reachable too. Indexing it up front is
        // what lets a `$ref` across documents resolve without the registry
        // being borrowed for the validator's whole life.
        if let Some(registry) = registry {
            for uri in registry.resources().map(str::to_owned).collect::<Vec<_>>() {
                if compiled.resources.contains_key(&uri) {
                    continue;
                }
                let Some(resource) = registry.resolve(&uri) else {
                    continue;
                };
                compiled.index(&uri, Arc::new(resource.clone()))?;
            }
        }

        Ok(compiled)
    }

    /// Records one resource: its anchors, its nested `$id`s, and its patterns.
    fn index(&mut self, uri: &str, document: Arc<Value>) -> Result<(), CompileError> {
        if self.resources.insert(uri.to_owned(), Arc::clone(&document)).is_none() {
            self.order.push(uri.to_owned());
        }

        // Collected first so the borrow of `document` ends before indexing the
        // nested resources it names.
        let mut patterns: Vec<(String, String)> = Vec::new();
        let mut anchors: Vec<(String, String, bool)> = Vec::new();
        let mut nested: Vec<(String, String)> = Vec::new();

        walk_subschemas(&document, |pointer, node| {
            let Some(object) = node.as_object() else {
                return;
            };

            // A nested `$id` below the root is its own resource.
            if !pointer.is_empty()
                && let Some(id) = object.get("$id").and_then(Value::as_str)
            {
                nested.push((pointer.to_owned(), id.to_owned()));
                return;
            }

            if let Some(pattern) = object.get("pattern").and_then(Value::as_str) {
                patterns.push((format!("{pointer}/pattern"), pattern.to_owned()));
            }
            if let Some(Value::Object(map)) = object.get("patternProperties") {
                for pattern in map.keys() {
                    patterns.push((format!("{pointer}/patternProperties"), pattern.clone()));
                }
            }
            if let Some(name) = object.get("$anchor").and_then(Value::as_str) {
                anchors.push((name.to_owned(), pointer.to_owned(), false));
            }
            if let Some(name) = object.get("$dynamicAnchor").and_then(Value::as_str) {
                anchors.push((name.to_owned(), pointer.to_owned(), true));
            }
        });

        for (location, pattern) in patterns {
            self.compile_pattern(&location, &pattern)?;
        }
        for (name, pointer, dynamic) in anchors {
            let key = (uri.to_owned(), name);
            match dynamic {
                // A `$dynamicAnchor` is also a plain-name target for an
                // ordinary `$ref`, per the 2020-12 core specification.
                true => {
                    self.dynamic_anchors.insert(key.clone(), pointer.clone());
                    self.anchors.entry(key).or_insert(pointer);
                }
                false => {
                    self.anchors.insert(key, pointer);
                }
            }
        }

        for (pointer, id) in nested {
            let resolved = uri::resolve(uri, &id).0;
            if self.resources.contains_key(&resolved) {
                continue;
            }
            let Some(subtree) = resolve_pointer(&document, &pointer) else {
                continue;
            };
            self.index(&resolved, Arc::new(subtree.clone()))?;
        }

        Ok(())
    }

    fn compile_pattern(&mut self, location: &str, pattern: &str) -> Result<(), CompileError> {
        if self.regexes.contains_key(pattern) {
            return Ok(());
        }
        match Regex::new(pattern) {
            Ok(compiled) => {
                self.regexes.insert(pattern.to_owned(), compiled);
                Ok(())
            }
            Err(error) => Err(CompileError::BadPattern {
                location: location.to_owned(),
                pattern: pattern.to_owned(),
                reason: error.to_string(),
            }),
        }
    }

    /// The root document.
    pub(crate) fn root(&self) -> &Value {
        &self.document
    }

    /// The base URI the root establishes.
    pub(crate) fn base(&self) -> &str {
        &self.base
    }

    /// A compiled pattern. `None` only for one the schema never declared, which
    /// the evaluator treats as an assertion it cannot make.
    pub(crate) fn regex(&self, pattern: &str) -> Option<&Regex> {
        self.regexes.get(pattern)
    }

    /// Resolves a reference written under `base`, returning the target and the
    /// base in force inside it.
    #[cfg(test)]
    pub(crate) fn resolve(&self, base: &str, target: &str, dynamic: bool) -> Option<(Value, String)> {
        self.resolve_in_scope(base, target, dynamic, &[])
            .map(|(value, base, _)| (value, base))
    }

    /// Resolves a reference, searching `scope` for a `$dynamicRef`.
    ///
    /// `scope` is the chain of resources the evaluation actually entered,
    /// outermost first. A `$dynamicRef` whose fragment names a
    /// `$dynamicAnchor` in the resource it initially resolves to is redirected
    /// to the **outermost** resource in that chain carrying the same anchor —
    /// which is what lets a recursive schema hand control back to whichever
    /// document started the evaluation, rather than to itself.
    ///
    /// When the initial target has no matching `$dynamicAnchor`, the keyword
    /// behaves as a plain `$ref` and the scope is not consulted at all.
    pub(crate) fn resolve_in_scope(
        &self,
        base: &str,
        target: &str,
        dynamic: bool,
        scope: &[String],
    ) -> Option<(Value, String, String)> {
        let (resource_uri, fragment) = uri::resolve(base, target);

        if dynamic && !uri::is_pointer_fragment(&fragment) {
            // Only a `$dynamicRef` that lands on a `$dynamicAnchor` takes the
            // dynamic path; otherwise it is an ordinary reference.
            let anchored = self
                .dynamic_anchors
                .contains_key(&(resource_uri.clone(), fragment.clone()));
            if anchored {
                for uri in scope {
                    let key = (uri.clone(), fragment.clone());
                    if let Some(pointer) = self.dynamic_anchors.get(&key) {
                        let resource = self.resources.get(uri)?;
                        let found = resolve_pointer(resource, pointer)?;
                        return Some((found.clone(), uri.clone(), pointer.clone()));
                    }
                }
            }
        }

        let resource = self.resources.get(&resource_uri)?;
        let pointer = match uri::is_pointer_fragment(&fragment) {
            // A fragment is part of a URI, so a pointer written in one is
            // percent-encoded and has to be decoded before it resolves.
            true => uri::decode_fragment(&fragment),
            false => {
                // A `$dynamicRef` that found no matching `$dynamicAnchor`
                // above behaves as an ordinary `$ref`, so it resolves against
                // the plain anchors — `$anchor` and `$dynamicAnchor` alike,
                // both of which are indexed there.
                let key = (resource_uri.clone(), fragment);
                self.anchors.get(&key)?.clone()
            }
        };

        let found = resolve_pointer(resource, &pointer)?;
        Some((found.clone(), resource_uri, pointer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Schema;

    fn compile(schema: &Schema) -> Result<Compiled, CompileError> {
        Compiled::new(Arc::new(schema.to_value()), None)
    }

    #[test]
    fn every_pattern_in_the_schema_is_compiled_up_front() {
        let schema = Schema::object()
            .property("a", Schema::string().pattern("^[a-z]+$"))
            .pattern_property("^x-", Schema::string())
            .build()
            .unwrap();

        let compiled = compile(&schema).unwrap();
        assert!(compiled.regex("^[a-z]+$").is_some());
        assert!(compiled.regex("^x-").is_some());
        assert!(compiled.regex("never declared").is_none());
    }

    #[test]
    fn a_pattern_that_will_not_compile_is_reported_with_where_it_is() {
        // `regex-lite` has no lookahead.
        let schema = Schema::object()
            .property("a", Schema::string().pattern("(?=foo)bar"))
            .build()
            .unwrap();

        let error = compile(&schema).unwrap_err();
        let CompileError::BadPattern {
            location, pattern, ..
        } = &error
        else {
            panic!("expected a pattern error, got {error:?}");
        };
        assert_eq!(location, "/properties/a/pattern");
        assert_eq!(pattern, "(?=foo)bar");
        assert!(error.to_string().contains("cannot compile"));
    }

    #[test]
    fn anchors_are_indexed_and_resolve() {
        let schema = Schema::object()
            .id("https://example.com/s")
            .def("A", Schema::string().anchor("letter-a"))
            .build()
            .unwrap();

        let compiled = compile(&schema).unwrap();
        let (by_anchor, _) = compiled
            .resolve("https://example.com/s", "#letter-a", false)
            .unwrap();
        let (by_pointer, _) = compiled
            .resolve("https://example.com/s", "#/$defs/A", false)
            .unwrap();
        assert_eq!(by_anchor, by_pointer);
    }

    #[test]
    fn a_dynamic_anchor_also_answers_a_plain_ref() {
        let schema = Schema::object()
            .id("https://example.com/s")
            .def("Node", Schema::object().dynamic_anchor("node"))
            .build()
            .unwrap();

        let compiled = compile(&schema).unwrap();
        assert!(
            compiled
                .resolve("https://example.com/s", "#node", false)
                .is_some()
        );
        assert!(
            compiled
                .resolve("https://example.com/s", "#node", true)
                .is_some()
        );
    }

    #[test]
    fn a_nested_id_becomes_its_own_resource() {
        let schema = Schema::object()
            .id("https://example.com/root")
            .def(
                "Inner",
                Schema::object()
                    .id("https://example.com/inner")
                    .def("Deep", Schema::string()),
            )
            .build()
            .unwrap();

        let compiled = compile(&schema).unwrap();
        assert!(
            compiled
                .resolve("https://example.com/inner", "#/$defs/Deep", false)
                .is_some()
        );
    }

    #[test]
    fn a_schema_with_no_id_gets_the_anonymous_base() {
        let schema = Schema::object()
            .def("A", Schema::string())
            .build()
            .unwrap();

        let compiled = compile(&schema).unwrap();
        assert_eq!(compiled.base(), ANONYMOUS);
        assert!(compiled.resolve(ANONYMOUS, "#/$defs/A", false).is_some());
    }

    #[test]
    fn a_reference_to_nothing_resolves_to_nothing() {
        let schema = Schema::object().build().unwrap();
        let compiled = compile(&schema).unwrap();
        assert!(compiled.resolve(ANONYMOUS, "#/$defs/Missing", false).is_none());
        assert!(
            compiled
                .resolve(ANONYMOUS, "https://elsewhere.example/x", false)
                .is_none()
        );
    }

    #[test]
    fn a_boolean_schema_compiles() {
        assert!(Compiled::new(Arc::new(serde_json::json!(true)), None).is_ok());
        assert!(Compiled::new(Arc::new(serde_json::json!(false)), None).is_ok());
    }

    #[test]
    fn something_that_is_not_a_schema_is_refused() {
        let error = Compiled::new(Arc::new(serde_json::json!([1, 2])), None).unwrap_err();
        assert!(matches!(error, CompileError::NotASchema { .. }));
    }
}

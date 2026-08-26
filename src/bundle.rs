//! Collapsing a set of schemas into one self-contained document.
//!
//! A schema split across files is pleasant to maintain and awkward to ship: the
//! consumer needs every document, and needs them reachable under the URIs the
//! `$ref`s name. Bundling turns that set into a single document that carries
//! everything it references, so it can be handed over as one file.
//!
//! ```
//! # use ash_jsonschema_builder::{Registry, Schema, bundle::bundle};
//! let address = Schema::object()
//!     .id("https://example.com/address")
//!     .required_property("city", Schema::string())
//!     .build()?;
//!
//! let user = Schema::object()
//!     .id("https://example.com/user")
//!     .required_property("address", Schema::reference("https://example.com/address"))
//!     .build()?;
//!
//! let mut registry = Registry::new();
//! registry.insert(&address)?;
//! registry.insert(&user)?;
//!
//! let bundled = bundle(&registry, "https://example.com/user")?;
//!
//! // One document, and nothing left pointing outside it.
//! let mut check = Registry::new();
//! check.insert(&bundled)?;
//! check.check_references()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## How it is done
//!
//! The method is the one the specification describes: each referenced document
//! is embedded whole under `$defs`, keeping its own `$id`. An embedded document
//! remains a schema resource in its own right, so the `$ref`s that named it
//! still resolve — to the copy now inside the bundle rather than to a document
//! that has to be fetched. No reference is rewritten, and no `$id` is invented.
//!
//! That is what makes bundling reversible: the embedded documents can be lifted
//! back out unchanged.
//!
//! ## What is left alone
//!
//! A reference that resolves *within* the root document needs no help and is
//! untouched. A reference that resolves nowhere is reported rather than
//! silently embedded as a gap — see [`BundleError::Unresolved`].

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::registry::Registry;
use crate::schema::Schema;
use crate::uri;
use crate::walk::walk_subschemas;

/// A bundle that could not be produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BundleError {
    /// The root URI names no resource in the registry.
    UnknownRoot {
        /// The URI asked for.
        uri: String,
    },
    /// A reference in the tree resolves to nothing, so the bundle would be
    /// incomplete.
    ///
    /// Run [`Registry::check_references`] first to see every such reference at
    /// once, rather than one at a time.
    Unresolved {
        /// The reference as written.
        target: String,
        /// The resource it was written in.
        resource: String,
    },
    /// The root document is a boolean schema, which has nowhere to put `$defs`.
    ///
    /// A boolean schema references nothing, so bundling one is never necessary.
    BooleanRoot,
}

impl std::fmt::Display for BundleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownRoot { uri } => {
                write!(f, "the registry holds no resource under `{uri}`")
            }
            Self::Unresolved { target, resource } => write!(
                f,
                "`{target}`, referenced from `{resource}`, resolves to nothing",
            ),
            Self::BooleanRoot => {
                f.write_str("a boolean schema cannot carry the `$defs` a bundle needs")
            }
        }
    }
}

impl std::error::Error for BundleError {}

/// The `$defs` name an embedded document is filed under.
///
/// Derived from the URI so a reader can see where each embedded document came
/// from, and made unique by suffixing when two URIs would collide.
fn embed_name(uri: &str, taken: &BTreeMap<String, String>) -> String {
    // The last non-empty path segment carries the meaning: for
    // `https://example.com/schemas/address`, that is `address`.
    let trimmed = uri.trim_end_matches('/');
    let stem = trimmed
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or(trimmed);

    let sanitized: String = stem
        .chars()
        .map(|character| match character.is_alphanumeric() || character == '-' {
            true => character,
            false => '_',
        })
        .collect();
    let base = match sanitized.is_empty() {
        true => "embedded".to_owned(),
        false => sanitized,
    };

    // A name already used by a *different* URI gets a numeric suffix.
    if taken.get(&base).is_none_or(|held| held == uri) {
        return base;
    }
    for suffix in 2.. {
        let candidate = format!("{base}_{suffix}");
        if taken.get(&candidate).is_none_or(|held| held == uri) {
            return candidate;
        }
    }
    unreachable!("the suffix search is unbounded")
}

/// Produces a self-contained document from the resource at `root`.
///
/// Every document reachable by `$ref` is embedded under `$defs`, keeping its
/// own `$id`, so the references that named it resolve inside the bundle. The
/// root's own `$defs` entries are preserved; embedded documents are added
/// alongside them.
///
/// ```
/// # use ash_jsonschema_builder::{Registry, Schema, bundle::bundle};
/// # let address = Schema::object().id("https://example.com/address").build().unwrap();
/// # let user = Schema::object()
/// #     .id("https://example.com/user")
/// #     .property("a", Schema::reference("https://example.com/address"))
/// #     .build().unwrap();
/// # let mut registry = Registry::new();
/// # registry.insert(&address).unwrap();
/// # registry.insert(&user).unwrap();
/// let bundled = bundle(&registry, "https://example.com/user")?;
///
/// // The embedded document keeps its `$id`, which is what makes the
/// // original reference still resolve.
/// let embedded = bundled.def("address").unwrap();
/// assert_eq!(
///     embedded.as_object().unwrap().id.as_deref(),
///     Some("https://example.com/address"),
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Errors
///
/// [`BundleError::UnknownRoot`] if `root` names nothing in the registry, and
/// [`BundleError::Unresolved`] if a reference in the tree resolves nowhere —
/// bundling an incomplete document would produce one that quietly does not
/// work.
pub fn bundle(registry: &Registry, root: &str) -> Result<Schema, BundleError> {
    let root_uri = uri::resource_of(root).to_owned();
    let root_document = registry
        .resolve(&root_uri)
        .ok_or_else(|| BundleError::UnknownRoot {
            uri: root_uri.clone(),
        })?
        .clone();

    if !root_document.is_object() {
        return Err(BundleError::BooleanRoot);
    }

    // Every external resource reachable from the root, breadth-first so the
    // names an embedded document gets do not depend on recursion order.
    let mut embedded: BTreeMap<String, String> = BTreeMap::new(); // name -> uri
    let mut by_uri: BTreeMap<String, Value> = BTreeMap::new();
    let mut queue = vec![(root_uri.clone(), root_document.clone())];
    let mut seen = vec![root_uri.clone()];

    while let Some((base, document)) = queue.pop() {
        for (target, resource) in references_in(&document, &base) {
            let (target_uri, _) = uri::resolve(&resource, &target);

            // A reference that stays inside a document already in the bundle
            // needs nothing done to it.
            if seen.contains(&target_uri) {
                continue;
            }

            let resolved =
                registry
                    .resolve(&target_uri)
                    .ok_or_else(|| BundleError::Unresolved {
                        target: target.clone(),
                        resource: resource.clone(),
                    })?;

            seen.push(target_uri.clone());
            by_uri.insert(target_uri.clone(), resolved.clone());
            queue.push((target_uri, resolved.clone()));
        }
    }

    for target_uri in by_uri.keys() {
        let name = embed_name(target_uri, &embedded);
        embedded.insert(name, target_uri.clone());
    }

    let mut bundled = root_document;
    if !embedded.is_empty() {
        let object = bundled
            .as_object_mut()
            .expect("the root was checked to be an object");
        let defs = object
            .entry("$defs")
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(defs) = defs.as_object_mut() else {
            // `$defs` holding a non-object is not a legal schema; leave it be
            // rather than overwriting what the author wrote.
            return Ok(Schema::from_value(bundled).expect("a schema round-trips through Value"));
        };
        for (name, target_uri) in &embedded {
            defs.insert(name.clone(), by_uri[target_uri].clone());
        }
    }

    Ok(Schema::from_value(bundled).expect("a schema round-trips through Value"))
}

/// Every `$ref` and `$dynamicRef` in a document, with the base each was written
/// under.
fn references_in(document: &Value, base: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut bases: Vec<(String, String)> = vec![(String::new(), base.to_owned())];

    walk_subschemas(document, |pointer, node| {
        let Some(object) = node.as_object() else {
            return;
        };

        // The base in force here is the nearest enclosing `$id`.
        let enclosing = bases
            .iter()
            .filter(|(at, _)| pointer.starts_with(at.as_str()))
            .max_by_key(|(at, _)| at.len())
            .map(|(_, uri)| uri.clone())
            .unwrap_or_else(|| base.to_owned());

        let here = match object.get("$id").and_then(Value::as_str) {
            Some(id) => {
                let resolved = uri::resolve(&enclosing, id).0;
                bases.push((pointer.to_owned(), resolved.clone()));
                resolved
            }
            None => enclosing,
        };

        for keyword in ["$ref", "$dynamicRef"] {
            if let Some(target) = object.get(keyword).and_then(Value::as_str) {
                found.push((target.to_owned(), here.clone()));
            }
        }
    });

    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry_of(schemas: &[&Schema]) -> Registry {
        let mut registry = Registry::new();
        for schema in schemas {
            registry.insert(schema).unwrap();
        }
        registry
    }

    #[test]
    fn a_document_with_no_external_references_is_returned_unchanged() {
        let schema = Schema::object()
            .id("https://example.com/s")
            .def("Name", Schema::string())
            .property("name", Schema::reference("#/$defs/Name"))
            .build()
            .unwrap();

        let registry = registry_of(&[&schema]);
        let bundled = bundle(&registry, "https://example.com/s").unwrap();

        assert_eq!(bundled, schema);
    }

    #[test]
    fn a_referenced_document_is_embedded_keeping_its_id() {
        let address = Schema::object()
            .id("https://example.com/address")
            .required_property("city", Schema::string())
            .build()
            .unwrap();
        let user = Schema::object()
            .id("https://example.com/user")
            .required_property("address", Schema::reference("https://example.com/address"))
            .build()
            .unwrap();

        let registry = registry_of(&[&address, &user]);
        let bundled = bundle(&registry, "https://example.com/user").unwrap();

        let embedded = bundled.def("address").expect("embedded under its stem");
        assert_eq!(
            embedded.as_object().unwrap().id.as_deref(),
            Some("https://example.com/address"),
        );
    }

    #[test]
    fn the_bundle_resolves_on_its_own() {
        let address = Schema::object()
            .id("https://example.com/address")
            .build()
            .unwrap();
        let user = Schema::object()
            .id("https://example.com/user")
            .property("address", Schema::reference("https://example.com/address"))
            .build()
            .unwrap();

        let bundled = bundle(&registry_of(&[&address, &user]), "https://example.com/user").unwrap();

        // The bundle alone, with nothing else present, has no dangling refs.
        let mut check = Registry::new();
        check.insert(&bundled).unwrap();
        check.check_references().unwrap();
    }

    #[test]
    fn a_transitive_reference_is_embedded_too() {
        let country = Schema::object()
            .id("https://example.com/country")
            .build()
            .unwrap();
        let address = Schema::object()
            .id("https://example.com/address")
            .property("country", Schema::reference("https://example.com/country"))
            .build()
            .unwrap();
        let user = Schema::object()
            .id("https://example.com/user")
            .property("address", Schema::reference("https://example.com/address"))
            .build()
            .unwrap();

        let bundled = bundle(
            &registry_of(&[&country, &address, &user]),
            "https://example.com/user",
        )
        .unwrap();

        assert!(bundled.def("address").is_some());
        assert!(bundled.def("country").is_some());

        let mut check = Registry::new();
        check.insert(&bundled).unwrap();
        check.check_references().unwrap();
    }

    #[test]
    fn the_roots_own_defs_survive_bundling() {
        let other = Schema::object()
            .id("https://example.com/other")
            .build()
            .unwrap();
        let root = Schema::object()
            .id("https://example.com/root")
            .def("Mine", Schema::string())
            .property("a", Schema::reference("#/$defs/Mine"))
            .property("b", Schema::reference("https://example.com/other"))
            .build()
            .unwrap();

        let bundled = bundle(&registry_of(&[&other, &root]), "https://example.com/root").unwrap();

        assert!(bundled.def("Mine").is_some());
        assert!(bundled.def("other").is_some());
    }

    #[test]
    fn two_documents_with_the_same_stem_get_distinct_names() {
        let first = Schema::object()
            .id("https://a.example/common")
            .title("first")
            .build()
            .unwrap();
        let second = Schema::object()
            .id("https://b.example/common")
            .title("second")
            .build()
            .unwrap();
        let root = Schema::object()
            .id("https://example.com/root")
            .property("a", Schema::reference("https://a.example/common"))
            .property("b", Schema::reference("https://b.example/common"))
            .build()
            .unwrap();

        let bundled = bundle(
            &registry_of(&[&first, &second, &root]),
            "https://example.com/root",
        )
        .unwrap();

        assert!(bundled.def("common").is_some());
        assert!(bundled.def("common_2").is_some());

        let mut check = Registry::new();
        check.insert(&bundled).unwrap();
        check.check_references().unwrap();
    }

    #[test]
    fn a_cycle_between_documents_terminates() {
        let a = Schema::object()
            .id("https://example.com/a")
            .property("b", Schema::reference("https://example.com/b"))
            .build()
            .unwrap();
        let b = Schema::object()
            .id("https://example.com/b")
            .property("a", Schema::reference("https://example.com/a"))
            .build()
            .unwrap();

        let bundled = bundle(&registry_of(&[&a, &b]), "https://example.com/a").unwrap();

        // `b` is embedded; `a` is the root and is not embedded inside itself.
        assert!(bundled.def("b").is_some());
        assert!(bundled.def("a").is_none());

        let mut check = Registry::new();
        check.insert(&bundled).unwrap();
        check.check_references().unwrap();
    }

    #[test]
    fn a_self_reference_needs_no_embedding() {
        let node = Schema::object()
            .id("https://example.com/node")
            .property("next", Schema::reference("https://example.com/node"))
            .build()
            .unwrap();

        let bundled = bundle(&registry_of(&[&node]), "https://example.com/node").unwrap();
        assert_eq!(bundled, node);
    }

    #[test]
    fn an_unknown_root_is_refused() {
        let registry = Registry::new();
        assert_eq!(
            bundle(&registry, "https://example.com/nope"),
            Err(BundleError::UnknownRoot {
                uri: "https://example.com/nope".to_owned()
            }),
        );
    }

    #[test]
    fn a_dangling_reference_is_refused_rather_than_bundled_around() {
        let root = Schema::object()
            .id("https://example.com/root")
            .property("a", Schema::reference("https://example.com/missing"))
            .build()
            .unwrap();

        let error = bundle(&registry_of(&[&root]), "https://example.com/root").unwrap_err();
        assert!(matches!(error, BundleError::Unresolved { .. }));
        assert!(error.to_string().contains("resolves to nothing"));
    }

    /// The properties that make a bundle worth shipping: it is a legal schema,
    /// it resolves with the originals gone, it survives being written out and
    /// read back, and bundling it again changes nothing.
    #[test]
    fn a_bundle_is_valid_portable_and_stable() {
        let country = Schema::object()
            .id("https://example.com/country")
            .required_property("code", Schema::string().length(2))
            .build()
            .unwrap();
        let address = Schema::object()
            .id("https://example.com/address")
            .required_property("country", Schema::reference("https://example.com/country"))
            .build()
            .unwrap();
        let user = Schema::object()
            .id("https://example.com/user")
            .required_property("address", Schema::reference("https://example.com/address"))
            .build()
            .unwrap();

        let registry = registry_of(&[&country, &address, &user]);
        let bundled = bundle(&registry, "https://example.com/user").unwrap();

        // A legal 2020-12 document.
        bundled.validate().unwrap();

        // Resolves with nothing else present — the originals are not consulted.
        let mut alone = Registry::new();
        alone.insert(&bundled).unwrap();
        alone.check_references().unwrap();
        assert_eq!(alone.len(), 3);

        // Survives the trip through text, which is how it would be shipped.
        let reparsed = Schema::parse(&bundled.to_json()).unwrap();
        let mut round_tripped = Registry::new();
        round_tripped.insert(&reparsed).unwrap();
        round_tripped.check_references().unwrap();
        assert_eq!(reparsed, bundled);

        // Bundling an already-bundled document is a no-op.
        let again = bundle(&alone, "https://example.com/user").unwrap();
        assert_eq!(again, bundled);
    }

    #[test]
    fn a_name_is_derived_from_the_uris_last_segment() {
        let taken = BTreeMap::new();
        assert_eq!(embed_name("https://example.com/address", &taken), "address");
        assert_eq!(embed_name("https://example.com/a/b/thing", &taken), "thing");
        assert_eq!(embed_name("https://example.com/trailing/", &taken), "trailing");
        // A segment that is not a usable name is sanitized.
        assert_eq!(embed_name("urn:example:thing", &taken), "urn_example_thing");
    }
}

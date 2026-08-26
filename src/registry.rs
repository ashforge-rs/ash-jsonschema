//! Resolving `$ref` — a set of schemas indexed by the URIs they declare.
//!
//! A `$ref` is a URI reference, and resolving one takes two things the schema
//! itself does not carry: a base URI to resolve *against*, and somewhere to
//! look the result up. A [`Registry`] is that somewhere. Schemas go in under
//! the `$id` they declare, their `$anchor`s and `$dynamicAnchor`s are indexed
//! as they are inserted, and a reference then resolves to the subschema it
//! names — or to nothing, which [`check_references`](Registry::check_references)
//! reports.
//!
//! ```
//! # use ash_jsonschema_builder::{Registry, Schema};
//! let schema = Schema::object()
//!     .id("https://example.com/user")
//!     .def("Name", Schema::string().min_length(1))
//!     .required_property("name", Schema::reference("#/$defs/Name"))
//!     .build()?;
//!
//! let mut registry = Registry::new();
//! registry.insert(&schema)?;
//!
//! let target = registry.resolve("https://example.com/user#/$defs/Name").unwrap();
//! assert_eq!(target.get("minLength"), Some(&serde_json::json!(1)));
//!
//! registry.check_references()?;   // every `$ref` lands somewhere
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## Nothing is fetched
//!
//! A registry resolves against what it was given and nothing else. A `$ref` to
//! a resource that was never inserted is a dangling reference, not a request to
//! go and get it — retrieval is the caller's policy, and keeping it out here is
//! what makes resolution offline, reproducible, and free of surprise I/O.
//!
//! ## What a registry does not do
//!
//! It resolves references; it does not validate instances. Resolving tells you
//! *which* schema a `$ref` names, not whether any particular JSON satisfies it.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::error::{ErrorKind, SchemaError, Violation};
use crate::meta::resolve_pointer;
use crate::schema::Schema;
use crate::uri;
use crate::walk::walk_subschemas;

/// The URI a schema inserted without an `$id` is filed under.
///
/// A document that declares no `$id` still has to be reachable, and its
/// internal `#/$defs/…` references still have to resolve, so it is given this
/// base rather than being refused.
pub const ANONYMOUS: &str = "urn:ash-jsonschema:anonymous";

/// One `$ref` found in an indexed schema, and where it was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    /// The resource the reference was written in, as a URI.
    pub resource: String,
    /// A JSON Pointer to the `$ref` keyword itself, within that resource.
    pub location: String,
    /// The reference exactly as written.
    pub target: String,
    /// Whether this is a `$dynamicRef` rather than a `$ref`.
    pub dynamic: bool,
}

impl Reference {
    /// The absolute URI this reference names, resolved against the resource it
    /// was written in.
    pub fn absolute(&self) -> String {
        let (resource, fragment) = uri::resolve(&self.resource, &self.target);
        match fragment.is_empty() {
            true => resource,
            false => format!("{resource}#{fragment}"),
        }
    }
}

/// A schema that could not be added to a registry.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistryError {
    /// Two schemas declared the same `$id`.
    ///
    /// Resolution would be ambiguous, so the second insert is refused rather
    /// than silently replacing the first. Use
    /// [`Registry::replace`] to overwrite deliberately.
    DuplicateId {
        /// The `$id` both schemas declared.
        id: String,
    },
    /// A schema declared an `$id` that is not usable as a base URI.
    ///
    /// The 2020-12 meta-schema already forbids a non-empty fragment on `$id`;
    /// this additionally refuses one that is empty or relative, because a
    /// registry has no document base to resolve it against.
    UnusableId {
        /// The `$id` as written.
        id: String,
        /// Why it cannot serve as a base.
        reason: &'static str,
    },
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateId { id } => {
                write!(f, "two schemas in the registry declare `$id` `{id}`")
            }
            Self::UnusableId { id, reason } => {
                write!(f, "`$id` `{id}` cannot be a base URI: {reason}")
            }
        }
    }
}

impl std::error::Error for RegistryError {}

/// One indexed schema resource.
#[derive(Clone, Debug)]
struct Resource {
    /// The document this resource lives in, shared with every other resource
    /// declared inside it.
    ///
    /// Shared rather than owned for two reasons: `serde_json::Value` clones
    /// and drops recursively, so copying a deeply nested subtree risks
    /// overflowing the stack, and a nested `$id` would otherwise duplicate
    /// everything beneath it.
    document: Arc<Value>,
    /// Where in `document` this resource begins. The empty pointer is the
    /// document root; a nested `$id` starts deeper in.
    base_pointer: String,
    /// `$anchor` name → JSON Pointer within this resource.
    anchors: HashMap<String, String>,
    /// `$dynamicAnchor` name → JSON Pointer within this resource.
    dynamic_anchors: HashMap<String, String>,
    /// Every `$ref` and `$dynamicRef` written anywhere in this resource.
    references: Vec<Reference>,
}

impl Resource {
    /// Resolves a pointer that is relative to this resource's own root.
    fn get(&self, pointer: &str) -> Option<&Value> {
        let full = format!("{}{pointer}", self.base_pointer);
        resolve_pointer(&self.document, &full)
    }
}

/// A set of schemas, indexed by URI, that `$ref`s resolve against.
///
/// Insertion order is the order resources were added, which
/// [`resolve_dynamic`](Registry::resolve_dynamic) uses when several resources
/// carry the same `$dynamicAnchor`.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    resources: HashMap<String, Resource>,
    /// Resource URIs in insertion order.
    order: Vec<String>,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Indexes a schema under the `$id` it declares.
    ///
    /// A schema with no `$id` is filed under [`ANONYMOUS`], so its own internal
    /// references still resolve. Nested `$id`s start their own resources and
    /// are indexed too, each resolved against the base it appears under.
    ///
    /// ```
    /// # use ash_jsonschema_builder::{Registry, Schema};
    /// let schema = Schema::object().id("https://example.com/a").build()?;
    ///
    /// let mut registry = Registry::new();
    /// registry.insert(&schema)?;
    ///
    /// assert!(registry.contains("https://example.com/a"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`RegistryError::DuplicateId`] if a resource with that URI is already
    /// present, and [`RegistryError::UnusableId`] if an `$id` cannot serve as a
    /// base URI.
    pub fn insert(&mut self, schema: &Schema) -> Result<(), RegistryError> {
        self.index(Arc::new(schema.to_value()), false)
    }

    /// Indexes a schema, replacing any resource already filed under the same
    /// URI instead of refusing.
    pub fn replace(&mut self, schema: &Schema) -> Result<(), RegistryError> {
        self.index(Arc::new(schema.to_value()), true)
    }

    /// Indexes a document that did not come from this crate's builders — one
    /// parsed from a file, or produced by another library.
    ///
    /// The document is taken as-is; nothing checks that it is a well-formed
    /// schema. Pair it with
    /// [`validate_document`](crate::validate_document) when the source is
    /// untrusted.
    pub fn insert_document(&mut self, document: &Value) -> Result<(), RegistryError> {
        self.index(Arc::new(document.clone()), false)
    }

    /// Indexes a document the registry takes shared ownership of.
    ///
    /// The other entry points copy the document they are handed;
    /// `serde_json::Value` clones recursively, so this is the one to reach for
    /// when a document may be deeply nested or is large enough that a copy
    /// matters. It is also how to index one document from several registries
    /// without paying for it twice.
    ///
    /// ```
    /// # use ash_jsonschema_builder::{Registry, Schema};
    /// # use std::sync::Arc;
    /// let document = Arc::new(Schema::object().id("https://example.com/a").build()?.to_value());
    ///
    /// let mut registry = Registry::new();
    /// registry.insert_shared(Arc::clone(&document))?;
    ///
    /// assert!(registry.contains("https://example.com/a"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn insert_shared(&mut self, document: Arc<Value>) -> Result<(), RegistryError> {
        self.index(document, false)
    }

    fn index(&mut self, document: Arc<Value>, replace: bool) -> Result<(), RegistryError> {
        // Every resource this document declares, keyed by URI. A nested `$id`
        // starts a new one, so a single document can contribute several.
        let mut found: Vec<(String, Resource)> = Vec::new();

        // The root's base comes from its own `$id`, or `ANONYMOUS` if it has
        // none. A nested `$id` resolves against the base enclosing it, which
        // requires knowing each subschema's enclosing resource — so the walk
        // records bases as it descends, keyed by pointer.
        let root_id = document.get("$id").and_then(Value::as_str);
        let root_base = match root_id {
            Some(id) => check_base(id)?,
            None => ANONYMOUS.to_owned(),
        };

        // pointer → the base URI in force at that pointer.
        let mut bases: HashMap<String, String> = HashMap::new();
        bases.insert(String::new(), root_base.clone());

        let mut collected: Vec<(String, String)> = Vec::new();
        let mut error = None;

        walk_subschemas(&document, |pointer, node| {
            if error.is_some() {
                return;
            }
            // The enclosing base is the one recorded for the nearest ancestor
            // that established it. The walk is depth-first and parents are
            // visited before children, so it is already present.
            let enclosing = nearest_base(&bases, pointer).unwrap_or_else(|| root_base.clone());

            let Some(id) = node.get("$id").and_then(Value::as_str) else {
                bases.insert(pointer.to_owned(), enclosing);
                return;
            };
            if pointer.is_empty() {
                // Already handled as the root base.
                bases.insert(pointer.to_owned(), enclosing);
                return;
            }

            // A nested `$id` resolves against the base it sits under.
            let (resolved, fragment) = uri::resolve(&enclosing, id);
            if !fragment.is_empty() {
                error = Some(RegistryError::UnusableId {
                    id: id.to_owned(),
                    reason: "a nested `$id` may not carry a fragment",
                });
                return;
            }
            bases.insert(pointer.to_owned(), resolved.clone());
            collected.push((resolved, pointer.to_owned()));
        });

        if let Some(error) = error {
            return Err(error);
        }

        // Every resource the document declares points into the one shared
        // document rather than owning a copy of its own subtree.
        let shared = document;
        found.push((
            root_base.clone(),
            build_resource(&root_base, Arc::clone(&shared), ""),
        ));
        for (uri, pointer) in collected {
            let resource = build_resource(&uri, Arc::clone(&shared), &pointer);
            found.push((uri, resource));
        }

        if !replace {
            for (uri, _) in &found {
                if self.resources.contains_key(uri) {
                    return Err(RegistryError::DuplicateId { id: uri.clone() });
                }
            }
        }

        for (uri, resource) in found {
            if self.resources.insert(uri.clone(), resource).is_none() {
                self.order.push(uri);
            }
        }
        Ok(())
    }

    /// Whether a resource is filed under `uri`.
    pub fn contains(&self, uri: &str) -> bool {
        self.resources.contains_key(uri::resource_of(uri))
    }

    /// The resource URIs held, in insertion order.
    pub fn resources(&self) -> impl Iterator<Item = &str> {
        self.order.iter().map(String::as_str)
    }

    /// The number of resources held.
    pub fn len(&self) -> usize {
        self.resources.len()
    }

    /// Whether the registry holds no resources.
    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }

    /// Resolves an absolute reference to the subschema it names.
    ///
    /// The fragment may be a JSON Pointer or a plain-name `$anchor`; both are
    /// looked up against the resource the URI names. Returns `None` if the
    /// resource is not held, or the fragment names nothing within it.
    ///
    /// ```
    /// # use ash_jsonschema_builder::{Registry, Schema};
    /// let schema = Schema::object()
    ///     .id("https://example.com/s")
    ///     .def("A", Schema::string().anchor("letter-a"))
    ///     .build()?;
    ///
    /// let mut registry = Registry::new();
    /// registry.insert(&schema)?;
    ///
    /// // By pointer, and by the anchor naming the same place.
    /// assert!(registry.resolve("https://example.com/s#/$defs/A").is_some());
    /// assert!(registry.resolve("https://example.com/s#letter-a").is_some());
    /// assert!(registry.resolve("https://example.com/s#/$defs/Missing").is_none());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn resolve(&self, reference: &str) -> Option<&Value> {
        let (resource_uri, fragment) = uri::resolve("", reference);
        self.resolve_in(&resource_uri, &fragment, false)
    }

    /// Resolves a reference written inside `base`, which relative references
    /// resolve against.
    pub fn resolve_from(&self, base: &str, reference: &str) -> Option<&Value> {
        let (resource_uri, fragment) = uri::resolve(base, reference);
        self.resolve_in(&resource_uri, &fragment, false)
    }

    /// Resolves a `$dynamicRef`.
    ///
    /// When the fragment is a plain name that the named resource carries as a
    /// `$dynamicAnchor`, the *outermost* resource holding that anchor wins —
    /// which is what lets a recursive schema hand control back to whichever
    /// document started the evaluation. "Outermost" here means earliest
    /// inserted, since a registry has no running evaluation to take a dynamic
    /// scope from.
    ///
    /// Full `$dynamicRef` semantics depend on the dynamic scope of an
    /// in-progress evaluation, which a registry does not have. This resolves
    /// the common recursive case correctly and falls back to
    /// [`resolve`](Self::resolve) otherwise.
    pub fn resolve_dynamic(&self, reference: &str) -> Option<&Value> {
        let (resource_uri, fragment) = uri::resolve("", reference);

        if !uri::is_pointer_fragment(&fragment) {
            let named_it = self
                .resources
                .get(&resource_uri)
                .is_some_and(|resource| resource.dynamic_anchors.contains_key(&fragment));

            if named_it {
                for uri in &self.order {
                    let resource = &self.resources[uri];
                    if let Some(pointer) = resource.dynamic_anchors.get(&fragment) {
                        return resource.get(pointer);
                    }
                }
            }
        }

        self.resolve_in(&resource_uri, &fragment, true)
    }

    fn resolve_in(&self, resource_uri: &str, fragment: &str, dynamic: bool) -> Option<&Value> {
        let resource = self.resources.get(resource_uri)?;

        let pointer = if uri::is_pointer_fragment(fragment) {
            fragment.to_owned()
        } else {
            let anchors = match dynamic {
                true => &resource.dynamic_anchors,
                false => &resource.anchors,
            };
            // An `$anchor` lookup falls back to the dynamic table: a
            // `$dynamicAnchor` is also a plain-name target for an ordinary
            // `$ref`, per the 2020-12 core specification.
            anchors
                .get(fragment)
                .or_else(|| resource.dynamic_anchors.get(fragment))
                .or_else(|| resource.anchors.get(fragment))?
                .clone()
        };

        resource.get(&pointer)
    }

    /// Every `$ref` and `$dynamicRef` written in the indexed schemas.
    pub fn references(&self) -> impl Iterator<Item = &Reference> {
        self.order
            .iter()
            .flat_map(|uri| self.resources[uri].references.iter())
    }

    /// Checks that every reference resolves to something the registry holds.
    ///
    /// This is the check `build()` deliberately does not run: a `$ref` naming a
    /// definition that does not exist is a legal 2020-12 *document*, so it is
    /// reported here rather than at build time. Each dangling reference becomes
    /// a [`Violation`] located at the `$ref` keyword that wrote it.
    ///
    /// ```
    /// # use ash_jsonschema_builder::{Registry, Schema};
    /// let schema = Schema::object()
    ///     .id("https://example.com/s")
    ///     .property("a", Schema::reference("#/$defs/Nope"))
    ///     .build()?;   // builds: the shape is legal
    ///
    /// let mut registry = Registry::new();
    /// registry.insert(&schema)?;
    ///
    /// let error = registry.check_references().unwrap_err();
    /// assert_eq!(error.first().location, "/properties/a/$ref");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// A [`SchemaError`] holding one violation per dangling reference, in
    /// insertion order.
    pub fn check_references(&self) -> Result<(), SchemaError> {
        let mut violations = Vec::new();

        for reference in self.references() {
            let absolute = reference.absolute();
            let found = match reference.dynamic {
                true => self.resolve_dynamic(&absolute).is_some(),
                false => self.resolve(&absolute).is_some(),
            };
            if found {
                continue;
            }

            let keyword = match reference.dynamic {
                true => "$dynamicRef",
                false => "$ref",
            };
            violations.push(Violation {
                location: format!("{}/{keyword}", reference.location),
                rule: format!("registry/{keyword}"),
                message: format!("`{}` resolves to nothing in the registry", reference.target),
            });
        }

        match violations.is_empty() {
            true => Ok(()),
            false => Err(SchemaError::of_kind(violations, ErrorKind::References)),
        }
    }
}

/// Rejects an `$id` that cannot serve as a base URI.
fn check_base(id: &str) -> Result<String, RegistryError> {
    let (resource, fragment) = match id.split_once('#') {
        Some((resource, fragment)) => (resource, fragment),
        None => (id, ""),
    };
    if !fragment.is_empty() {
        return Err(RegistryError::UnusableId {
            id: id.to_owned(),
            reason: "an `$id` may not carry a non-empty fragment",
        });
    }
    if resource.is_empty() {
        return Err(RegistryError::UnusableId {
            id: id.to_owned(),
            reason: "an `$id` may not be empty",
        });
    }
    if !resource.contains("://") && !resource.contains(':') {
        return Err(RegistryError::UnusableId {
            id: id.to_owned(),
            reason: "a registry has no document base to resolve a relative `$id` against",
        });
    }
    Ok(resource.to_owned())
}

/// The base recorded for the nearest ancestor of `pointer`, including itself.
fn nearest_base(bases: &HashMap<String, String>, pointer: &str) -> Option<String> {
    let mut candidate = pointer;
    loop {
        if let Some(base) = bases.get(candidate) {
            return Some(base.clone());
        }
        match candidate.rfind('/') {
            Some(0) | None => return bases.get("").cloned(),
            Some(index) => candidate = &candidate[..index],
        }
    }
}

/// Indexes one resource's anchors and references.
///
/// The walk stops descending at a nested `$id`, since everything under it
/// belongs to that resource instead — except at the root, which is this
/// resource's own declaration.
fn build_resource(uri: &str, document: Arc<Value>, base_pointer: &str) -> Resource {
    let root = resolve_pointer(&document, base_pointer)
        .expect("the walk produced this pointer, so it resolves");
    let mut anchors = HashMap::new();
    let mut dynamic_anchors = HashMap::new();
    let mut references = Vec::new();

    walk_subschemas(root, |pointer, node| {
        // A nested `$id` below the root starts another resource, which is
        // indexed separately; its contents are not this resource's.
        if !pointer.is_empty() && node.get("$id").is_some() {
            return;
        }
        let Some(object) = node.as_object() else {
            return;
        };

        if let Some(name) = object.get("$anchor").and_then(Value::as_str) {
            anchors.insert(name.to_owned(), pointer.to_owned());
        }
        if let Some(name) = object.get("$dynamicAnchor").and_then(Value::as_str) {
            dynamic_anchors.insert(name.to_owned(), pointer.to_owned());
        }
        if let Some(target) = object.get("$ref").and_then(Value::as_str) {
            references.push(Reference {
                resource: uri.to_owned(),
                location: pointer.to_owned(),
                target: target.to_owned(),
                dynamic: false,
            });
        }
        if let Some(target) = object.get("$dynamicRef").and_then(Value::as_str) {
            references.push(Reference {
                resource: uri.to_owned(),
                location: pointer.to_owned(),
                target: target.to_owned(),
                dynamic: true,
            });
        }
    });

    Resource {
        document,
        base_pointer: base_pointer.to_owned(),
        anchors,
        dynamic_anchors,
        references,
    }
}

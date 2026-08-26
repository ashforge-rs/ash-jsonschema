//! Comparing two versions of a schema, and saying what the change would break.
//!
//! A schema is a contract between whoever writes the data and whoever reads it,
//! and a change to it lands on those two differently — often in opposite
//! directions. Adding a required property rejects data that used to be
//! accepted, so it breaks **writers**; but a reader that already handled the
//! property is untroubled. Removing a required property is the mirror image:
//! writers are free to carry on, while a reader that relied on the field now
//! finds it missing.
//!
//! There is no single axis that captures both, so [`Change`] carries the effect
//! on each, and the caller asks the question it actually has:
//!
//! ```
//! # use ash_jsonschema::{Schema, diff::{diff, Role}};
//! let v1 = Schema::object()
//!     .required_property("id", Schema::string())
//!     .build()?;
//!
//! let v2 = Schema::object()
//!     .required_property("id", Schema::string())
//!     .required_property("email", Schema::string())
//!     .build()?;
//!
//! let report = diff(&v1, &v2);
//!
//! // Data written for v1 has no `email`, so it no longer validates.
//! assert!(report.is_breaking_for(Role::Writer));
//! // A reader built for v1 simply ignores the new field.
//! assert!(!report.is_breaking_for(Role::Reader));
//! # Ok::<(), ash_jsonschema::SchemaError>(())
//! ```
//!
//! ## What a diff can and cannot know
//!
//! Classification is conservative and syntactic. Where the effect of a change
//! is not decidable by inspection — two different `pattern`s, a reordered
//! `oneOf`, a rewritten `$ref` target — the change is reported as
//! [`Effect::Unknown`] rather than guessed at, and
//! [`is_breaking_for`](Report::is_breaking_for) counts it as breaking. A diff
//! that quietly called an unclear change safe would be worse than useless in
//! the gate it is meant to guard.
//!
//! `$ref`s are compared by the target they name; the schema they point at is
//! not followed. [`diff_resolved`] follows them through a [`Registry`] when
//! that is what you want.

use serde_json::Value;

use crate::registry::Registry;
use crate::schema::Schema;
use crate::uri;

/// Which side of the contract a change is being judged from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// Whoever produces the data. A change breaks a writer when data that used
    /// to be accepted no longer is.
    Writer,
    /// Whoever consumes the data. A change breaks a reader when data it must
    /// now handle is outside what it was built for.
    Reader,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Writer => "writer",
            Self::Reader => "reader",
        })
    }
}

/// What a change does to one side of the contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Effect {
    /// Nothing this side relied on has changed.
    Compatible,
    /// This side is broken by the change.
    Breaking,
    /// The effect could not be determined by inspection.
    ///
    /// Treated as breaking by [`Report::is_breaking_for`], because a change
    /// nobody can classify is not one to wave through a gate.
    Unknown,
}

impl Effect {
    /// Whether this effect should stop a release.
    ///
    /// [`Unknown`](Effect::Unknown) counts as breaking: the point of the gate
    /// is to catch what a person needs to look at.
    pub fn is_breaking(&self) -> bool {
        matches!(self, Self::Breaking | Self::Unknown)
    }
}

impl std::fmt::Display for Effect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Compatible => "compatible",
            Self::Breaking => "breaking",
            Self::Unknown => "unknown",
        })
    }
}

/// What kind of change was found.
///
/// The variants name the change as an author would describe it, which is what
/// makes a report readable. Each one determines the two effects a [`Change`]
/// carries.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChangeKind {
    /// A property was added to `properties` and to `required`.
    AddedRequiredProperty(String),
    /// A property was added to `properties` but not to `required`.
    AddedOptionalProperty(String),
    /// A property was removed from `properties`.
    RemovedProperty(String),
    /// An existing property became required.
    PropertyBecameRequired(String),
    /// An existing property stopped being required.
    PropertyBecameOptional(String),
    /// A bound was tightened — a higher `minimum`, a lower `maxItems`, and so
    /// on. Fewer instances validate than before.
    Narrowed {
        /// The keyword that changed.
        keyword: String,
        /// Its value before.
        before: Value,
        /// Its value after.
        after: Value,
    },
    /// A bound was loosened. More instances validate than before.
    Widened {
        /// The keyword that changed.
        keyword: String,
        /// Its value before.
        before: Value,
        /// Its value after.
        after: Value,
    },
    /// A keyword changed in a way whose effect cannot be read off the values.
    Changed {
        /// The keyword that changed.
        keyword: String,
        /// Its value before, absent if the keyword was added.
        before: Option<Value>,
        /// Its value after, absent if the keyword was removed.
        after: Option<Value>,
    },
    /// The permitted `type`s changed.
    TypeChanged {
        /// The types before.
        before: Value,
        /// The types after.
        after: Value,
    },
    /// A schema changed between its boolean and object forms, or a subschema
    /// appeared or vanished where one side had none.
    FormChanged,
    /// An annotation changed: `title`, `description`, `$comment`, `examples`,
    /// `default`, `deprecated`. Never breaking on its own.
    AnnotationChanged(String),
}

impl ChangeKind {
    /// The effect of this kind of change on each side of the contract.
    fn effects(&self) -> (Effect, Effect) {
        use Effect::{Breaking, Compatible, Unknown};

        match self {
            // A writer must now supply it; a reader that did not know the field
            // is unaffected by its arrival.
            Self::AddedRequiredProperty(_) | Self::PropertyBecameRequired(_) => {
                (Breaking, Compatible)
            }
            // Nobody is forced to do anything.
            Self::AddedOptionalProperty(_) => (Compatible, Compatible),
            // A reader that relied on it no longer finds it. A writer that was
            // sending it may now be rejected if the object closed, but that
            // shows up as its own change.
            Self::RemovedProperty(_) | Self::PropertyBecameOptional(_) => (Compatible, Breaking),
            // Fewer instances are accepted: what a writer sends may be refused,
            // while a reader sees a subset of what it already handled.
            Self::Narrowed { .. } => (Breaking, Compatible),
            // More instances are accepted: a writer keeps working, a reader now
            // faces values outside what it was built for.
            Self::Widened { .. } => (Compatible, Breaking),
            // Not decidable by inspection, so nobody is told it is safe.
            Self::Changed { .. } | Self::TypeChanged { .. } | Self::FormChanged => {
                (Unknown, Unknown)
            }
            Self::AnnotationChanged(_) => (Compatible, Compatible),
        }
    }
}

impl std::fmt::Display for ChangeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AddedRequiredProperty(name) => write!(f, "added required property `{name}`"),
            Self::AddedOptionalProperty(name) => write!(f, "added optional property `{name}`"),
            Self::RemovedProperty(name) => write!(f, "removed property `{name}`"),
            Self::PropertyBecameRequired(name) => write!(f, "`{name}` became required"),
            Self::PropertyBecameOptional(name) => write!(f, "`{name}` became optional"),
            Self::Narrowed {
                keyword,
                before,
                after,
            } => write!(f, "`{keyword}` tightened from {before} to {after}"),
            Self::Widened {
                keyword,
                before,
                after,
            } => write!(f, "`{keyword}` loosened from {before} to {after}"),
            Self::Changed {
                keyword,
                before,
                after,
            } => match (before, after) {
                (Some(before), Some(after)) => {
                    write!(f, "`{keyword}` changed from {before} to {after}")
                }
                (None, Some(after)) => write!(f, "`{keyword}` added as {after}"),
                (Some(before), None) => write!(f, "`{keyword}` removed (was {before})"),
                (None, None) => write!(f, "`{keyword}` changed"),
            },
            Self::TypeChanged { before, after } => {
                write!(f, "`type` changed from {before} to {after}")
            }
            Self::FormChanged => f.write_str("the schema form changed"),
            Self::AnnotationChanged(keyword) => write!(f, "`{keyword}` changed"),
        }
    }
}

/// One difference between two schemas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// A JSON Pointer to the subschema the change was found in. The empty
    /// string is the root.
    pub location: String,
    /// What changed.
    pub kind: ChangeKind,
    /// What it does to whoever writes the data.
    pub for_writer: Effect,
    /// What it does to whoever reads it.
    pub for_reader: Effect,
}

impl Change {
    fn new(location: &str, kind: ChangeKind) -> Self {
        let (for_writer, for_reader) = kind.effects();
        Self {
            location: location.to_owned(),
            kind,
            for_writer,
            for_reader,
        }
    }

    /// The effect on one side.
    pub fn effect_for(&self, role: Role) -> Effect {
        match role {
            Role::Writer => self.for_writer,
            Role::Reader => self.for_reader,
        }
    }
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let location = match self.location.is_empty() {
            true => "(root)",
            false => &self.location,
        };
        write!(
            f,
            "{location}: {} [writer: {}, reader: {}]",
            self.kind, self.for_writer, self.for_reader
        )
    }
}

/// Every difference between two schemas.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The changes found, in document order.
    pub changes: Vec<Change>,
}

impl Report {
    /// Whether the two schemas were identical.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Whether anything here breaks `role`.
    ///
    /// [`Effect::Unknown`] counts as breaking — see [`Effect::is_breaking`].
    ///
    /// ```
    /// # use ash_jsonschema::{Schema, diff::{diff, Role}};
    /// let v1 = Schema::object().property("a", Schema::string()).build()?;
    /// let v2 = Schema::object().build()?;
    ///
    /// let report = diff(&v1, &v2);
    /// assert!(report.is_breaking_for(Role::Reader));   // `a` is gone
    /// assert!(!report.is_breaking_for(Role::Writer));  // sending less is fine
    /// # Ok::<(), ash_jsonschema::SchemaError>(())
    /// ```
    pub fn is_breaking_for(&self, role: Role) -> bool {
        self.changes
            .iter()
            .any(|change| change.effect_for(role).is_breaking())
    }

    /// The changes that break `role`.
    pub fn breaking_for(&self, role: Role) -> impl Iterator<Item = &Change> {
        self.changes
            .iter()
            .filter(move |change| change.effect_for(role).is_breaking())
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.changes.is_empty() {
            return f.write_str("the schemas are identical");
        }
        let count = self.changes.len();
        let plural = match count {
            1 => "change",
            _ => "changes",
        };
        write!(f, "{count} {plural}")?;
        for change in &self.changes {
            write!(f, "\n  - {change}")?;
        }
        Ok(())
    }
}

/// Compares two schemas, `$ref`s taken at face value.
///
/// Two `$ref`s are equal when they name the same target; where they point is
/// not followed. Use [`diff_resolved`] to compare through a registry instead.
///
/// ```
/// # use ash_jsonschema::{Schema, diff::diff};
/// let v1 = Schema::object().property("age", Schema::integer().minimum(0)).build()?;
/// let v2 = Schema::object().property("age", Schema::integer().minimum(18)).build()?;
///
/// let report = diff(&v1, &v2);
/// assert_eq!(report.changes.len(), 1);
/// assert_eq!(report.changes[0].location, "/properties/age");
/// # Ok::<(), ash_jsonschema::SchemaError>(())
/// ```
pub fn diff(before: &Schema, after: &Schema) -> Report {
    let mut differ = Differ {
        registries: None,
        changes: Vec::new(),
        in_progress: Vec::new(),
        bases: (String::new(), String::new()),
    };
    differ.compare(&before.to_value(), &after.to_value(), "");
    Report {
        changes: differ.changes,
    }
}

/// Compares two schemas, following `$ref`s through a registry for each side.
///
/// A reference is resolved before comparing, so a change inside a shared
/// definition is reported at the place that uses it rather than only where the
/// definition sits.
///
/// **Each side needs its own registry.** The two schemas are different versions
/// of the same document, so a `$ref` written in `before` must resolve against
/// what `before` knew and a `$ref` in `after` against what `after` knows —
/// resolving both through one registry would compare a definition with itself
/// and report no change. Pass the same registry twice only when the referenced
/// documents genuinely did not change.
///
/// ```
/// # use ash_jsonschema::{Registry, Schema, diff::{diff_resolved, Role}};
/// let version = |length: u64| {
///     Schema::object()
///         .id("https://example.com/s")
///         .def("Name", Schema::string().min_length(length))
///         .property("name", Schema::reference("#/$defs/Name"))
///         .build()
///         .unwrap()
/// };
/// let (v1, v2) = (version(1), version(5));
///
/// let mut before = Registry::new();
/// before.insert(&v1).unwrap();
/// let mut after = Registry::new();
/// after.insert(&v2).unwrap();
///
/// let report = diff_resolved(&before, &v1, &after, &v2);
///
/// // The tightened bound is attributed to the property that uses it.
/// assert!(report.changes.iter().any(|change| change.location == "/properties/name"));
/// assert!(report.is_breaking_for(Role::Writer));
/// ```
///
/// A reference that does not resolve is compared by target, as [`diff`] would.
/// A pair of references already being compared further up the stack is taken as
/// equal, which is what stops a recursive schema from diffing forever.
pub fn diff_resolved(
    before_registry: &Registry,
    before: &Schema,
    after_registry: &Registry,
    after: &Schema,
) -> Report {
    let mut differ = Differ {
        registries: Some((before_registry, after_registry)),
        changes: Vec::new(),
        in_progress: Vec::new(),
        bases: (String::new(), String::new()),
    };
    differ.compare(&before.to_value(), &after.to_value(), "");
    Report {
        changes: differ.changes,
    }
}

/// Keywords that annotate rather than assert. A change to one of these can
/// never make an instance valid or invalid.
const ANNOTATIONS: [&str; 8] = [
    "$comment",
    "default",
    "deprecated",
    "description",
    "examples",
    "readOnly",
    "title",
    "writeOnly",
];

/// Numeric keywords whose *increase* narrows the schema.
const NARROWS_WHEN_HIGHER: [&str; 6] = [
    "minLength",
    "minItems",
    "minProperties",
    "minContains",
    "minimum",
    "exclusiveMinimum",
];

/// Numeric keywords whose *decrease* narrows the schema.
const NARROWS_WHEN_LOWER: [&str; 6] = [
    "maxLength",
    "maxItems",
    "maxProperties",
    "maxContains",
    "maximum",
    "exclusiveMaximum",
];

struct Differ<'r> {
    /// The registries references resolve against: one for each side, since the
    /// two schemas are different versions of the same document.
    registries: Option<(&'r Registry, &'r Registry)>,
    changes: Vec<Change>,
    /// Reference pairs currently being compared, so a cycle terminates.
    in_progress: Vec<(String, String)>,
    /// The base URI in force on each side, which a relative `$ref` resolves
    /// against. A nested `$id` replaces it for the subtree beneath.
    bases: (String, String),
}

impl Differ<'_> {
    fn report(&mut self, location: &str, kind: ChangeKind) {
        self.changes.push(Change::new(location, kind));
    }

    fn compare(&mut self, before: &Value, after: &Value, location: &str) {
        // Two identical subschemas differ only if a `$ref` in them leads
        // somewhere that changed — which is exactly the case a version bump
        // produces, since the reference itself is usually left alone.
        if before == after && !(self.resolving() && mentions_reference(before)) {
            return;
        }

        // Following a `$ref` is opt-in, and only when both sides have one.
        if let Some(resolved) = self.follow(before, after) {
            let (before_target, after_target, key) = resolved;
            if self.in_progress.contains(&key) {
                // Already comparing this pair further up: assume equal, which
                // is what terminates a recursive schema.
                return;
            }
            self.in_progress.push(key);
            self.compare(&before_target, &after_target, location);
            self.in_progress.pop();
            return;
        }

        let (Some(before_map), Some(after_map)) = (before.as_object(), after.as_object()) else {
            // One side is a boolean schema, or the two forms differ.
            self.report(location, ChangeKind::FormChanged);
            return;
        };

        // An `$id` here rebases every relative `$ref` beneath it.
        let outer = self.bases.clone();
        self.bases = (
            rebase(&outer.0, before_map),
            rebase(&outer.1, after_map),
        );

        self.compare_properties(before_map, after_map, location);
        self.compare_keywords(before_map, after_map, location);

        self.bases = outer;
    }

    /// Whether references are being followed.
    fn resolving(&self) -> bool {
        self.registries.is_some()
    }

    /// Resolves both sides when each carries a `$ref` and a registry is set.
    #[allow(clippy::type_complexity)]
    fn follow(&self, before: &Value, after: &Value) -> Option<(Value, Value, (String, String))> {
        let (before_registry, after_registry) = self.registries?;
        let before_ref = before.get("$ref").and_then(Value::as_str)?;
        let after_ref = after.get("$ref").and_then(Value::as_str)?;

        // Each side resolves against the base in force where it was written,
        // so the usual relative form — `#/$defs/Name` — lands in the right
        // document rather than nowhere.
        let before_target = before_registry
            .resolve_from(&self.bases.0, before_ref)?
            .clone();
        let after_target = after_registry.resolve_from(&self.bases.1, after_ref)?.clone();

        let key = (
            absolute(&self.bases.0, before_ref),
            absolute(&self.bases.1, after_ref),
        );
        Some((before_target, after_target, key))
    }

    /// `properties` and `required`, which have to be read together: whether a
    /// property is required is not visible from `properties` alone.
    fn compare_properties(
        &mut self,
        before: &serde_json::Map<String, Value>,
        after: &serde_json::Map<String, Value>,
        location: &str,
    ) {
        let empty = serde_json::Map::new();
        let before_properties = before
            .get("properties")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let after_properties = after
            .get("properties")
            .and_then(Value::as_object)
            .unwrap_or(&empty);

        let before_required = required_names(before);
        let after_required = required_names(after);

        for (name, before_schema) in before_properties {
            let was_required = before_required.contains(name);
            match after_properties.get(name) {
                Some(after_schema) => {
                    let is_required = after_required.contains(name);
                    match (was_required, is_required) {
                        (false, true) => self.report(
                            location,
                            ChangeKind::PropertyBecameRequired(name.clone()),
                        ),
                        (true, false) => self.report(
                            location,
                            ChangeKind::PropertyBecameOptional(name.clone()),
                        ),
                        _ => {}
                    }
                    let nested = format!(
                        "{location}/properties/{}",
                        crate::meta::escape_pointer_token(name)
                    );
                    self.compare(before_schema, after_schema, &nested);
                }
                None => self.report(location, ChangeKind::RemovedProperty(name.clone())),
            }
        }

        for name in after_properties.keys() {
            if before_properties.contains_key(name) {
                continue;
            }
            let kind = match after_required.contains(name) {
                true => ChangeKind::AddedRequiredProperty(name.clone()),
                false => ChangeKind::AddedOptionalProperty(name.clone()),
            };
            self.report(location, kind);
        }

        // A name in `required` with no matching `properties` entry still binds.
        for name in &after_required {
            if after_properties.contains_key(name) || before_required.contains(name) {
                continue;
            }
            self.report(
                location,
                ChangeKind::AddedRequiredProperty(name.clone()),
            );
        }
        for name in &before_required {
            if before_properties.contains_key(name) || after_required.contains(name) {
                continue;
            }
            self.report(
                location,
                ChangeKind::PropertyBecameOptional(name.clone()),
            );
        }
    }

    /// A map of named subschemas: compare shared names in place, and report
    /// the rest as added or removed.
    ///
    /// Unlike `properties`, a name here is not a field of the instance, so an
    /// addition or removal is a structural change whose effect depends on who
    /// referenced it — reported as [`ChangeKind::Changed`], which is
    /// [`Effect::Unknown`].
    fn compare_schema_map(
        &mut self,
        before: &serde_json::Map<String, Value>,
        after: &serde_json::Map<String, Value>,
        keyword: &str,
        location: &str,
    ) {
        for (name, before_schema) in before {
            let token = crate::meta::escape_pointer_token(name);
            match after.get(name) {
                Some(after_schema) => self.compare(
                    before_schema,
                    after_schema,
                    &format!("{location}/{keyword}/{token}"),
                ),
                None => self.report(
                    location,
                    ChangeKind::Changed {
                        keyword: format!("{keyword}/{name}"),
                        before: Some(before_schema.clone()),
                        after: None,
                    },
                ),
            }
        }

        for (name, after_schema) in after {
            if before.contains_key(name) {
                continue;
            }
            self.report(
                location,
                ChangeKind::Changed {
                    keyword: format!("{keyword}/{name}"),
                    before: None,
                    after: Some(after_schema.clone()),
                },
            );
        }
    }

    /// Every keyword other than the two handled above.
    fn compare_keywords(
        &mut self,
        before: &serde_json::Map<String, Value>,
        after: &serde_json::Map<String, Value>,
        location: &str,
    ) {
        let mut names: Vec<&String> = before.keys().chain(after.keys()).collect();
        names.sort();
        names.dedup();

        for name in names {
            let keyword = name.as_str();
            if matches!(keyword, "properties" | "required") {
                continue;
            }

            let before_value = before.get(keyword);
            let after_value = after.get(keyword);
            if before_value == after_value {
                continue;
            }

            if ANNOTATIONS.contains(&keyword) {
                self.report(
                    location,
                    ChangeKind::AnnotationChanged(keyword.to_owned()),
                );
                continue;
            }

            // A keyword holding a subschema is compared as one, so the change
            // is attributed to where it actually is rather than reported as an
            // opaque blob at the enclosing schema.
            if crate::walk::SUBSCHEMA.contains(&keyword)
                && let (Some(before_schema), Some(after_schema)) = (before_value, after_value)
            {
                self.compare(
                    before_schema,
                    after_schema,
                    &format!("{location}/{keyword}"),
                );
                continue;
            }

            // `$defs` holds definitions that assert nothing on their own —
            // only a `$ref` reaching one gives it effect. When references are
            // being followed, the change is reported at each use site instead,
            // so comparing the definitions here as well would report it twice.
            if keyword == "$defs" && self.resolving() {
                continue;
            }

            // The same for a map of named subschemas — `$defs`,
            // `patternProperties`, `dependentSchemas`.
            if crate::walk::SUBSCHEMA_MAP.contains(&keyword)
                && let (Some(Value::Object(before_map)), Some(Value::Object(after_map))) =
                    (before_value, after_value)
            {
                self.compare_schema_map(before_map, after_map, keyword, location);
                continue;
            }

            // And for a positional list — `allOf`, `anyOf`, `oneOf`,
            // `prefixItems`. Only when the length matches: a list that grew or
            // shrank changes which branches apply, and pairing by index would
            // invent changes that are not there.
            if crate::walk::SUBSCHEMA_LIST.contains(&keyword)
                && let (Some(Value::Array(before_list)), Some(Value::Array(after_list))) =
                    (before_value, after_value)
                && before_list.len() == after_list.len()
            {
                for (index, (before_item, after_item)) in
                    before_list.iter().zip(after_list).enumerate()
                {
                    self.compare(
                        before_item,
                        after_item,
                        &format!("{location}/{keyword}/{index}"),
                    );
                }
                continue;
            }

            if keyword == "type" {
                self.report(
                    location,
                    ChangeKind::TypeChanged {
                        before: before_value.cloned().unwrap_or(Value::Null),
                        after: after_value.cloned().unwrap_or(Value::Null),
                    },
                );
                continue;
            }

            if let Some(kind) = bound_change(keyword, before_value, after_value) {
                self.report(location, kind);
                continue;
            }

            self.report(
                location,
                ChangeKind::Changed {
                    keyword: keyword.to_owned(),
                    before: before_value.cloned(),
                    after: after_value.cloned(),
                },
            );
        }
    }
}

/// The base a subschema establishes, or the one it inherits.
fn rebase(outer: &str, schema: &serde_json::Map<String, Value>) -> String {
    match schema.get("$id").and_then(Value::as_str) {
        Some(id) => uri::resolve(outer, id).0,
        None => outer.to_owned(),
    }
}

/// A reference resolved against a base, fragment included, for cycle detection.
fn absolute(base: &str, reference: &str) -> String {
    let (resource, fragment) = uri::resolve(base, reference);
    match fragment.is_empty() {
        true => resource,
        false => format!("{resource}#{fragment}"),
    }
}

/// Whether a subschema, or anything beneath it, carries a `$ref`.
///
/// Used to decide whether two textually identical subschemas still need
/// comparing: they do exactly when a reference inside them might now lead
/// somewhere different.
fn mentions_reference(schema: &Value) -> bool {
    let mut found = false;
    crate::walk::walk_subschemas(schema, |_, node| {
        found = found || node.get("$ref").is_some();
    });
    found
}

/// The names in `required`, as a set.
fn required_names(schema: &serde_json::Map<String, Value>) -> Vec<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Classifies a change to a numeric bound, when both sides are numbers.
///
/// Adding a bound that was absent narrows; removing one widens. Anything the
/// values cannot settle — a non-numeric value, an unrecognised keyword —
/// returns `None` so the caller reports it as an unclassified change.
fn bound_change(
    keyword: &str,
    before: Option<&Value>,
    after: Option<&Value>,
) -> Option<ChangeKind> {
    let higher_narrows = NARROWS_WHEN_HIGHER.contains(&keyword);
    let lower_narrows = NARROWS_WHEN_LOWER.contains(&keyword);
    if !higher_narrows && !lower_narrows {
        return None;
    }

    let describe = |narrowed: bool, before: Value, after: Value| match narrowed {
        true => ChangeKind::Narrowed {
            keyword: keyword.to_owned(),
            before,
            after,
        },
        false => ChangeKind::Widened {
            keyword: keyword.to_owned(),
            before,
            after,
        },
    };

    match (before, after) {
        // A bound appearing where there was none can only narrow.
        (None, Some(after)) if after.is_number() => {
            Some(describe(true, Value::Null, after.clone()))
        }
        // A bound disappearing can only widen.
        (Some(before), None) if before.is_number() => {
            Some(describe(false, before.clone(), Value::Null))
        }
        (Some(before), Some(after)) => {
            let (left, right) = (before.as_f64()?, after.as_f64()?);
            let narrowed = match higher_narrows {
                true => right > left,
                false => right < left,
            };
            Some(describe(narrowed, before.clone(), after.clone()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(report: &Report) -> Vec<&ChangeKind> {
        report.changes.iter().map(|change| &change.kind).collect()
    }

    #[test]
    fn an_identical_schema_reports_nothing() {
        let schema = Schema::object()
            .required_property("id", Schema::string())
            .build()
            .unwrap();

        let report = diff(&schema, &schema);
        assert!(report.is_empty());
        assert!(!report.is_breaking_for(Role::Writer));
        assert!(!report.is_breaking_for(Role::Reader));
        assert_eq!(report.to_string(), "the schemas are identical");
    }

    #[test]
    fn adding_a_required_property_breaks_writers_only() {
        let before = Schema::object()
            .required_property("id", Schema::string())
            .build()
            .unwrap();
        let after = Schema::object()
            .required_property("id", Schema::string())
            .required_property("email", Schema::string())
            .build()
            .unwrap();

        let report = diff(&before, &after);
        assert_eq!(
            kinds(&report),
            [&ChangeKind::AddedRequiredProperty("email".into())]
        );
        assert!(report.is_breaking_for(Role::Writer));
        assert!(!report.is_breaking_for(Role::Reader));
    }

    #[test]
    fn adding_an_optional_property_breaks_nobody() {
        let before = Schema::object().build().unwrap();
        let after = Schema::object()
            .property("nickname", Schema::string())
            .build()
            .unwrap();

        let report = diff(&before, &after);
        assert_eq!(
            kinds(&report),
            [&ChangeKind::AddedOptionalProperty("nickname".into())]
        );
        assert!(!report.is_breaking_for(Role::Writer));
        assert!(!report.is_breaking_for(Role::Reader));
    }

    #[test]
    fn removing_a_property_breaks_readers_only() {
        let before = Schema::object()
            .property("legacy", Schema::string())
            .build()
            .unwrap();
        let after = Schema::object().build().unwrap();

        let report = diff(&before, &after);
        assert_eq!(
            kinds(&report),
            [&ChangeKind::RemovedProperty("legacy".into())]
        );
        assert!(!report.is_breaking_for(Role::Writer));
        assert!(report.is_breaking_for(Role::Reader));
    }

    #[test]
    fn changing_whether_a_property_is_required_swaps_which_side_breaks() {
        let optional = Schema::object()
            .property("a", Schema::string())
            .build()
            .unwrap();
        let required = Schema::object()
            .required_property("a", Schema::string())
            .build()
            .unwrap();

        let tightened = diff(&optional, &required);
        assert_eq!(
            kinds(&tightened),
            [&ChangeKind::PropertyBecameRequired("a".into())]
        );
        assert!(tightened.is_breaking_for(Role::Writer));
        assert!(!tightened.is_breaking_for(Role::Reader));

        let loosened = diff(&required, &optional);
        assert_eq!(
            kinds(&loosened),
            [&ChangeKind::PropertyBecameOptional("a".into())]
        );
        assert!(!loosened.is_breaking_for(Role::Writer));
        assert!(loosened.is_breaking_for(Role::Reader));
    }

    #[test]
    fn a_tighter_bound_narrows_and_a_looser_one_widens() {
        let loose = Schema::integer().minimum(0).build().unwrap();
        let tight = Schema::integer().minimum(18).build().unwrap();

        let narrowed = diff(&loose, &tight);
        assert!(matches!(
            narrowed.changes[0].kind,
            ChangeKind::Narrowed { .. }
        ));
        assert!(narrowed.is_breaking_for(Role::Writer));
        assert!(!narrowed.is_breaking_for(Role::Reader));

        let widened = diff(&tight, &loose);
        assert!(matches!(
            widened.changes[0].kind,
            ChangeKind::Widened { .. }
        ));
        assert!(!widened.is_breaking_for(Role::Writer));
        assert!(widened.is_breaking_for(Role::Reader));
    }

    #[test]
    fn maximum_bounds_narrow_in_the_other_direction() {
        let loose = Schema::string().max_length(100).build().unwrap();
        let tight = Schema::string().max_length(10).build().unwrap();

        assert!(matches!(
            diff(&loose, &tight).changes[0].kind,
            ChangeKind::Narrowed { .. }
        ));
        assert!(matches!(
            diff(&tight, &loose).changes[0].kind,
            ChangeKind::Widened { .. }
        ));
    }

    #[test]
    fn adding_a_bound_narrows_and_removing_one_widens() {
        let unbounded = Schema::string().build().unwrap();
        let bounded = Schema::string().min_length(1).build().unwrap();

        assert!(matches!(
            diff(&unbounded, &bounded).changes[0].kind,
            ChangeKind::Narrowed { .. }
        ));
        assert!(matches!(
            diff(&bounded, &unbounded).changes[0].kind,
            ChangeKind::Widened { .. }
        ));
    }

    #[test]
    fn an_annotation_change_never_breaks_anyone() {
        let before = Schema::string().title("Before").build().unwrap();
        let after = Schema::string()
            .title("After")
            .description("now explained")
            .build()
            .unwrap();

        let report = diff(&before, &after);
        assert_eq!(report.changes.len(), 2);
        assert!(!report.is_breaking_for(Role::Writer));
        assert!(!report.is_breaking_for(Role::Reader));
    }

    #[test]
    fn an_unclassifiable_change_is_unknown_and_counts_as_breaking() {
        let before = Schema::string().pattern("^a").build().unwrap();
        let after = Schema::string().pattern("^b").build().unwrap();

        let report = diff(&before, &after);
        assert_eq!(report.changes[0].for_writer, Effect::Unknown);
        assert_eq!(report.changes[0].for_reader, Effect::Unknown);
        assert!(report.is_breaking_for(Role::Writer));
        assert!(report.is_breaking_for(Role::Reader));
    }

    #[test]
    fn a_type_change_is_reported_as_one() {
        let before = Schema::string().build().unwrap();
        let after = Schema::integer().build().unwrap();

        let report = diff(&before, &after);
        assert!(matches!(
            report.changes[0].kind,
            ChangeKind::TypeChanged { .. }
        ));
        assert!(report.is_breaking_for(Role::Writer));
    }

    #[test]
    fn a_change_inside_a_property_is_located_there() {
        let before = Schema::object()
            .property("age", Schema::integer().minimum(0))
            .build()
            .unwrap();
        let after = Schema::object()
            .property("age", Schema::integer().minimum(18))
            .build()
            .unwrap();

        let report = diff(&before, &after);
        assert_eq!(report.changes.len(), 1);
        assert_eq!(report.changes[0].location, "/properties/age");
    }

    #[test]
    fn a_change_under_a_subschema_keyword_is_located_there() {
        let before = Schema::array()
            .items(Schema::string().min_length(1))
            .build()
            .unwrap();
        let after = Schema::array()
            .items(Schema::string().min_length(5))
            .build()
            .unwrap();

        let report = diff(&before, &after);
        assert_eq!(report.changes[0].location, "/items");
    }

    #[test]
    fn a_boolean_schema_opposite_an_object_is_a_form_change() {
        let before = Schema::object()
            .additional_properties(false)
            .build()
            .unwrap();
        let after = Schema::object()
            .additional_properties(Schema::string())
            .build()
            .unwrap();

        let report = diff(&before, &after);
        assert_eq!(kinds(&report), [&ChangeKind::FormChanged]);
        assert!(report.is_breaking_for(Role::Writer));
    }

    #[test]
    fn a_required_name_with_no_property_entry_still_counts() {
        let before = Schema::object().build().unwrap();
        let after = Schema::object().required(["ghost"]).build().unwrap();

        let report = diff(&before, &after);
        assert_eq!(
            kinds(&report),
            [&ChangeKind::AddedRequiredProperty("ghost".into())]
        );
        assert!(report.is_breaking_for(Role::Writer));
    }

    #[test]
    fn breaking_for_lists_only_what_breaks_that_side() {
        let before = Schema::object()
            .property("gone", Schema::string())
            .build()
            .unwrap();
        let after = Schema::object()
            .required_property("added", Schema::string())
            .build()
            .unwrap();

        let report = diff(&before, &after);
        assert_eq!(report.changes.len(), 2);
        assert_eq!(report.breaking_for(Role::Writer).count(), 1);
        assert_eq!(report.breaking_for(Role::Reader).count(), 1);
    }

    #[test]
    fn display_lists_each_change_with_both_effects() {
        let before = Schema::object().build().unwrap();
        let after = Schema::object()
            .required_property("id", Schema::string())
            .build()
            .unwrap();

        let text = diff(&before, &after).to_string();
        assert!(text.starts_with("1 change"));
        assert!(text.contains("added required property `id`"));
        assert!(text.contains("writer: breaking"));
        assert!(text.contains("reader: compatible"));
    }

    #[test]
    fn refs_are_compared_by_target_unless_a_registry_is_given() {
        let before = Schema::builder().reference("#/$defs/A").build().unwrap();
        let after = Schema::builder().reference("#/$defs/B").build().unwrap();

        let report = diff(&before, &after);
        assert_eq!(report.changes.len(), 1);
        assert!(matches!(
            report.changes[0].kind,
            ChangeKind::Changed { .. }
        ));

        // The same target on both sides is no change at all.
        assert!(diff(&before, &before).is_empty());
    }
}

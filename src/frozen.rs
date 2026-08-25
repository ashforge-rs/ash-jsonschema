//! A schema that cannot be modified after it is built.
//!
//! [`Schema`] is an ordinary owned value with public fields, so whoever holds
//! one can still reach in and change it — which is the right default while a
//! document is being assembled, and the wrong one once it is finished and
//! shared. [`FrozenSchema`] closes that off: it hands out `&Schema` and nothing
//! more, so the schema inside it is fixed for as long as it exists.
//!
//! Freezing protects the *instance*, not the information. A mutable copy is
//! always a [`thaw`](FrozenSchema::thaw) away, and taking one leaves the frozen
//! original untouched.

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use serde::{Serialize, Serializer};

use crate::schema::Schema;

/// An immutable, cheaply shared [`Schema`].
///
/// Produced by [`Builder::build_frozen`](crate::Builder::build_frozen) or
/// [`Schema::freeze`]. Cloning shares the one underlying schema rather than
/// copying it, and the result is `Send + Sync`, so a schema built once at
/// startup can be handed to as many workers as you like.
///
/// Reading works exactly as it does on a `Schema`, through `Deref`:
///
/// ```
/// # use ash_jsonschema_builder::Schema;
/// let frozen = Schema::object()
///     .required_property("id", Schema::string())
///     .build_frozen()
///     .unwrap();
///
/// assert_eq!(frozen.to_json(), r#"{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}"#);
/// assert!(frozen.as_object().unwrap().required.is_some());
/// ```
///
/// Writing does not. This compiles:
///
/// ```
/// # use ash_jsonschema_builder::Schema;
/// let mut schema = Schema::string().build().unwrap();
/// if let Schema::Object(object) = &mut schema {
///     object.title = Some("changed".into());
/// }
/// ```
///
/// and the same thing through a `FrozenSchema` does not:
///
/// ```compile_fail
/// # use ash_jsonschema_builder::Schema;
/// let mut frozen = Schema::string().build_frozen().unwrap();
/// if let Schema::Object(object) = &mut *frozen {
///     object.title = Some("changed".into());
/// }
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct FrozenSchema {
    schema: Arc<Schema>,
}

impl FrozenSchema {
    /// Freezes a schema.
    ///
    /// This does not check the schema against the meta-schema — freezing and
    /// validating are separate concerns.
    /// [`Builder::build_frozen`](crate::Builder::build_frozen) does both.
    pub fn new(schema: Schema) -> Self {
        Self {
            schema: Arc::new(schema),
        }
    }

    /// The schema inside, borrowed.
    pub fn as_schema(&self) -> &Schema {
        &self.schema
    }

    /// Clones a mutable copy out.
    ///
    /// The frozen schema is unaffected — this hands back a separate value that
    /// happens to start out identical.
    ///
    /// ```
    /// # use ash_jsonschema_builder::Schema;
    /// let frozen = Schema::string().build_frozen().unwrap();
    ///
    /// let mut thawed = frozen.thaw();
    /// if let Schema::Object(object) = &mut thawed {
    ///     object.title = Some("mine now".into());
    /// }
    ///
    /// assert_eq!(thawed.as_object().unwrap().title.as_deref(), Some("mine now"));
    /// assert_eq!(frozen.as_object().unwrap().title, None);
    /// ```
    pub fn thaw(&self) -> Schema {
        Schema::clone(&self.schema)
    }

    /// Whether two handles share one underlying schema, as opposed to holding
    /// equal copies of two.
    ///
    /// ```
    /// # use ash_jsonschema_builder::Schema;
    /// let frozen = Schema::string().build_frozen().unwrap();
    /// let shared = frozen.clone();
    /// let separate = Schema::string().build_frozen().unwrap();
    ///
    /// assert!(frozen.is_same_as(&shared));
    /// assert!(!frozen.is_same_as(&separate));
    /// assert_eq!(frozen, separate); // still equal by value
    /// ```
    pub fn is_same_as(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.schema, &other.schema)
    }
}

impl Deref for FrozenSchema {
    type Target = Schema;

    fn deref(&self) -> &Schema {
        &self.schema
    }
}

impl AsRef<Schema> for FrozenSchema {
    fn as_ref(&self) -> &Schema {
        &self.schema
    }
}

impl From<Schema> for FrozenSchema {
    fn from(value: Schema) -> Self {
        Self::new(value)
    }
}

impl From<FrozenSchema> for Schema {
    /// Thaws, cloning the schema out of the handle.
    fn from(value: FrozenSchema) -> Self {
        value.thaw()
    }
}

impl Serialize for FrozenSchema {
    /// Identical to serializing the schema inside — freezing is not visible in
    /// the rendered document.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.schema.serialize(serializer)
    }
}

impl fmt::Display for FrozenSchema {
    /// Writes the indented JSON form, as [`Schema`] does.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.schema.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clone_shares_rather_than_copies() {
        let frozen = Schema::string().build_frozen().unwrap();
        let clone = frozen.clone();

        assert!(Arc::ptr_eq(&frozen.schema, &clone.schema));
        assert_eq!(Arc::strong_count(&frozen.schema), 2);
    }

    #[test]
    fn thawing_detaches_from_the_frozen_original() {
        let frozen = Schema::object()
            .property("a", Schema::string())
            .build_frozen()
            .unwrap();
        let mut thawed = frozen.thaw();

        if let Schema::Object(object) = &mut thawed {
            object.properties = None;
            object.title = Some("detached".into());
        }

        assert_eq!(thawed.as_object().unwrap().properties, None);
        assert!(frozen.as_object().unwrap().properties.is_some());
        assert_eq!(Arc::strong_count(&frozen.schema), 1);
    }

    #[test]
    fn it_renders_exactly_as_the_schema_inside_does() {
        let schema = Schema::object().title("Same").build_unchecked();
        let frozen = FrozenSchema::new(schema.clone());

        assert_eq!(frozen.to_json(), schema.to_json());
        assert_eq!(frozen.to_value(), schema.to_value());
        assert_eq!(frozen.to_string(), schema.to_string());
        assert_eq!(
            serde_json::to_string(&frozen).unwrap(),
            serde_json::to_string(&schema).unwrap()
        );
    }

    #[test]
    fn it_converts_both_ways_and_reads_through_deref() {
        let frozen = FrozenSchema::from(Schema::string().build_unchecked());
        assert_eq!(frozen.as_schema(), frozen.as_ref());
        // `Deref` reaches every read-only method `Schema` has.
        assert_eq!(
            frozen.as_object().unwrap().types,
            Schema::string()
                .build_unchecked()
                .as_object()
                .unwrap()
                .types
        );

        let thawed: Schema = frozen.clone().into();
        assert_eq!(thawed, *frozen);
    }

    #[test]
    fn equality_is_by_value_not_by_handle() {
        let one = Schema::integer().minimum(0).build_frozen().unwrap();
        let two = Schema::integer().minimum(0).build_frozen().unwrap();

        assert_eq!(one, two);
        assert!(!one.is_same_as(&two));
        assert!(one.is_same_as(&one.clone()));
    }

    #[test]
    fn it_is_shareable_across_threads() {
        let frozen = Schema::object().required(["a"]).build_frozen().unwrap();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let frozen = frozen.clone();
                std::thread::spawn(move || frozen.to_json())
            })
            .collect();

        for handle in handles {
            assert_eq!(handle.join().unwrap(), frozen.to_json());
        }
    }
}

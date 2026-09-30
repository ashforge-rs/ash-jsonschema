//! Keywords that only mean something for an object instance.
//!
//! Note the division of labour the specification draws: `properties` says what
//! a property must look like *if present*, and `required` — a separate list of
//! names — says it must be there at all. [`required_property`] writes both from
//! one call, which is what most callers mean.
//!
//! [`required_property`]: Builder::required_property

use crate::builder::{Builder, kind};
use crate::map::OrderedMap;
use crate::schema::Schema;

impl Builder<kind::Object> {
    /// Adds one entry to `properties`. Says nothing about whether the property
    /// must be present — see [`required`](Self::required) and
    /// [`required_property`](Self::required_property).
    pub fn property(mut self, name: impl Into<String>, schema: impl Into<Schema>) -> Self {
        self.inner
            .properties
            .get_or_insert_with(OrderedMap::new)
            .insert(name, schema.into());
        self
    }

    /// Adds several entries to `properties`.
    pub fn properties<N, S>(mut self, properties: impl IntoIterator<Item = (N, S)>) -> Self
    where
        N: Into<String>,
        S: Into<Schema>,
    {
        let target = self.inner.properties.get_or_insert_with(OrderedMap::new);
        for (name, schema) in properties {
            target.insert(name, schema.into());
        }
        self
    }

    /// Adds one entry to `properties` **and** its name to `required`.
    ///
    /// ```
    /// # use ash_jsonschema::Schema;
    /// let schema = Schema::object().required_property("id", Schema::integer()).build().unwrap();
    /// assert_eq!(
    ///     schema.to_json(),
    ///     r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#,
    /// );
    /// ```
    pub fn required_property(self, name: impl Into<String>, schema: impl Into<Schema>) -> Self {
        let name = name.into();
        self.property(name.clone(), schema).required([name])
    }

    /// Adds several entries to `properties`, marking every one required.
    ///
    /// ```
    /// # use ash_jsonschema::Schema;
    /// let schema = Schema::object()
    ///     .required_properties([("id", Schema::string()), ("email", Schema::string())])
    ///     .build()?;
    ///
    /// assert_eq!(schema.as_object().unwrap().required.as_ref().unwrap(), &["id", "email"]);
    /// # Ok::<(), ash_jsonschema::SchemaError>(())
    /// ```
    pub fn required_properties<N, S>(self, properties: impl IntoIterator<Item = (N, S)>) -> Self
    where
        N: Into<String>,
        S: Into<Schema>,
    {
        properties
            .into_iter()
            .fold(self, |builder, (name, schema)| {
                builder.required_property(name, schema)
            })
    }

    /// Adds one entry to `properties` and marks it required when `required` is
    /// true — for a field whose optionality is decided at run time.
    ///
    /// ```
    /// # use ash_jsonschema::Schema;
    /// # let mandatory = false;
    /// let schema = Schema::object()
    ///     .property_maybe_required("nickname", Schema::string(), mandatory)
    ///     .build()?;
    ///
    /// assert_eq!(schema.as_object().unwrap().required, None);
    /// # Ok::<(), ash_jsonschema::SchemaError>(())
    /// ```
    pub fn property_maybe_required(
        self,
        name: impl Into<String>,
        schema: impl Into<Schema>,
        required: bool,
    ) -> Self {
        if required {
            self.required_property(name, schema)
        } else {
            self.property(name, schema)
        }
    }

    /// Closes the object: `additionalProperties: false`.
    ///
    /// Shorthand for the call spelled out, which is what most schemas that
    /// close an object mean.
    ///
    /// ```
    /// # use ash_jsonschema::Schema;
    /// assert_eq!(
    ///     Schema::object().closed().build()?.to_json(),
    ///     r#"{"type":"object","additionalProperties":false}"#,
    /// );
    /// # Ok::<(), ash_jsonschema::SchemaError>(())
    /// ```
    pub fn closed(self) -> Self {
        self.additional_properties(false)
    }

    /// Appends names to `required`, skipping any already listed.
    pub fn required(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let target = self.inner.required.get_or_insert_with(Vec::new);
        for name in names {
            let name = name.into();
            if !target.contains(&name) {
                target.push(name);
            }
        }
        self
    }

    /// Adds one entry to `patternProperties`, keyed by an unanchored ECMA-262
    /// regular expression over property names.
    pub fn pattern_property(
        mut self,
        pattern: impl Into<String>,
        schema: impl Into<Schema>,
    ) -> Self {
        self.inner
            .pattern_properties
            .get_or_insert_with(OrderedMap::new)
            .insert(pattern, schema.into());
        self
    }

    /// Sets `additionalProperties` — the schema for properties that neither
    /// `properties` nor `patternProperties` matched.
    ///
    /// Pass `false` to close the object. Note this only sees the two keywords
    /// *in this same schema object*: to close an object assembled through
    /// `allOf` or `$ref`, use
    /// [`unevaluated_properties`](Self::unevaluated_properties) instead.
    pub fn additional_properties(mut self, schema: impl Into<Schema>) -> Self {
        self.inner.additional_properties = Some(Box::new(schema.into()));
        self
    }

    /// Sets `propertyNames` — a schema every property *name* must match. The
    /// instance handed to it is always a string.
    pub fn property_names(mut self, schema: impl Into<Schema>) -> Self {
        self.inner.property_names = Some(Box::new(schema.into()));
        self
    }

    /// Sets `minProperties`.
    pub fn min_properties(mut self, count: u64) -> Self {
        self.inner.min_properties = Some(count);
        self
    }

    /// Sets `maxProperties`.
    pub fn max_properties(mut self, count: u64) -> Self {
        self.inner.max_properties = Some(count);
        self
    }

    /// Adds one entry to `dependentRequired`: if `name` is present, every name
    /// in `requires` must be too.
    pub fn dependent_required(
        mut self,
        name: impl Into<String>,
        requires: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let requires = requires.into_iter().map(Into::into).collect();
        self.inner
            .dependent_required
            .get_or_insert_with(OrderedMap::new)
            .insert(name, requires);
        self
    }

    /// Adds one entry to `dependentSchemas`: if `name` is present, the whole
    /// instance must also match `schema`.
    pub fn dependent_schema(mut self, name: impl Into<String>, schema: impl Into<Schema>) -> Self {
        self.inner
            .dependent_schemas
            .get_or_insert_with(OrderedMap::new)
            .insert(name, schema.into());
        self
    }

    /// Sets `unevaluatedProperties` — applies to properties that no other
    /// keyword, here or through a `$ref` or an in-place applicator, evaluated.
    ///
    /// This is how a composed object is closed: `additionalProperties` cannot
    /// see what an `allOf` branch already accounted for, and this can.
    pub fn unevaluated_properties(mut self, schema: impl Into<Schema>) -> Self {
        self.inner.unevaluated_properties = Some(Box::new(schema.into()));
        self
    }
}

#[cfg(test)]
mod tests {
    use crate::schema::Schema;

    #[test]
    fn properties_keep_declaration_order() {
        let schema = Schema::object()
            .property("z", Schema::string())
            .property("a", Schema::string())
            .build()
            .unwrap();

        let names: Vec<_> = schema
            .as_object()
            .unwrap()
            .properties
            .as_ref()
            .unwrap()
            .keys()
            .collect();
        assert_eq!(names, ["z", "a"]);
    }

    #[test]
    fn re_declaring_a_property_replaces_it_in_place() {
        let schema = Schema::object()
            .property("a", Schema::string())
            .property("b", Schema::string())
            .property("a", Schema::integer())
            .build()
            .unwrap();

        let properties = schema.as_object().unwrap().properties.as_ref().unwrap();
        assert_eq!(properties.keys().collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(
            properties.get("a"),
            Some(&Schema::integer().build().unwrap())
        );
    }

    #[test]
    fn required_names_do_not_duplicate() {
        let schema = Schema::object()
            .required_property("id", Schema::string())
            .required(["id", "name"])
            .required(["name"])
            .build()
            .unwrap();

        assert_eq!(
            schema.as_object().unwrap().required.as_ref().unwrap(),
            &["id", "name"]
        );
    }

    #[test]
    fn properties_accepts_a_batch() {
        let schema = Schema::object()
            .properties([("a", Schema::string()), ("b", Schema::string())])
            .build()
            .unwrap();

        assert_eq!(
            schema
                .as_object()
                .unwrap()
                .properties
                .as_ref()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn dependencies_render_with_their_specified_names() {
        let schema = Schema::object()
            .dependent_required("card_number", ["billing_address"])
            .dependent_schema("card_number", Schema::object().required(["cvv"]))
            .build()
            .unwrap();

        assert_eq!(
            schema.to_json(),
            r#"{"type":"object","dependentRequired":{"card_number":["billing_address"]},"dependentSchemas":{"card_number":{"type":"object","required":["cvv"]}}}"#,
        );
    }

    #[test]
    fn an_object_can_be_closed_by_either_keyword() {
        let closed = Schema::object()
            .additional_properties(false)
            .build()
            .unwrap();
        assert_eq!(
            closed.to_json(),
            r#"{"type":"object","additionalProperties":false}"#
        );

        let composed = Schema::object()
            .unevaluated_properties(false)
            .build()
            .unwrap();
        assert_eq!(
            composed.to_json(),
            r#"{"type":"object","unevaluatedProperties":false}"#
        );
    }

    #[test]
    fn property_names_and_counts_render_together() {
        let schema = Schema::object()
            .pattern_property("^x-", Schema::string())
            .property_names(Schema::string().pattern("^[a-z-]+$"))
            .min_properties(1)
            .max_properties(8)
            .build()
            .unwrap();

        assert_eq!(
            schema.to_json(),
            r#"{"type":"object","patternProperties":{"^x-":{"type":"string"}},"propertyNames":{"type":"string","pattern":"^[a-z-]+$"},"maxProperties":8,"minProperties":1}"#,
        );
    }
}

//! Keywords that only mean something for an array instance.
//!
//! 2020-12 splits the old two-faced `items` keyword in two: `prefixItems`
//! validates the leading elements positionally, and `items` applies to
//! everything past that prefix. A schema that sets only `items` constrains
//! every element, which is the common case.

use crate::builder::{Builder, kind};
use crate::schema::Schema;

impl Builder<kind::Array> {
    /// Sets `items` — the schema every element past `prefixItems` must match.
    ///
    /// Pass `false` to forbid any element beyond the positional prefix.
    pub fn items(mut self, schema: impl Into<Schema>) -> Self {
        self.inner.items = Some(Box::new(schema.into()));
        self
    }

    /// Appends one schema to `prefixItems`, constraining the next position.
    pub fn prefix_item(mut self, schema: impl Into<Schema>) -> Self {
        self.inner
            .prefix_items
            .get_or_insert_with(Vec::new)
            .push(schema.into());
        self
    }

    /// Appends several schemas to `prefixItems`.
    ///
    /// ```
    /// # use ash_jsonschema::Schema;
    /// // A latitude/longitude pair, and nothing else.
    /// let schema = Schema::array()
    ///     .prefix_items([Schema::number().range(-90, 90), Schema::number().range(-180, 180)])
    ///     .items(false)
    ///     .build().unwrap();
    /// # assert!(schema.to_json().contains(r#""items":false"#));
    /// ```
    pub fn prefix_items(mut self, schemas: impl IntoIterator<Item = impl Into<Schema>>) -> Self {
        let target = self.inner.prefix_items.get_or_insert_with(Vec::new);
        target.extend(schemas.into_iter().map(Into::into));
        self
    }

    /// Sets `contains` — at least one element must match, unless
    /// [`min_contains`](Self::min_contains) says otherwise.
    pub fn contains(mut self, schema: impl Into<Schema>) -> Self {
        self.inner.contains = Some(Box::new(schema.into()));
        self
    }

    /// Sets `minContains`. Only meaningful alongside `contains`; `0` makes
    /// `contains` satisfiable by an array with no match at all.
    pub fn min_contains(mut self, count: u64) -> Self {
        self.inner.min_contains = Some(count);
        self
    }

    /// Sets `maxContains`. Only meaningful alongside `contains`.
    pub fn max_contains(mut self, count: u64) -> Self {
        self.inner.max_contains = Some(count);
        self
    }

    /// Sets `minItems`.
    pub fn min_items(mut self, count: u64) -> Self {
        self.inner.min_items = Some(count);
        self
    }

    /// Sets `maxItems`.
    pub fn max_items(mut self, count: u64) -> Self {
        self.inner.max_items = Some(count);
        self
    }

    /// Sets `uniqueItems` — no two elements may be equal under JSON equality.
    pub fn unique_items(mut self, unique: bool) -> Self {
        self.inner.unique_items = Some(unique);
        self
    }

    /// Sets `unevaluatedItems` — applies to elements that no other keyword,
    /// here or through a `$ref` or an in-place applicator, evaluated.
    pub fn unevaluated_items(mut self, schema: impl Into<Schema>) -> Self {
        self.inner.unevaluated_items = Some(Box::new(schema.into()));
        self
    }
}

#[cfg(test)]
mod tests {
    use crate::schema::Schema;

    #[test]
    fn items_alone_constrains_every_element() {
        let schema = Schema::array()
            .items(Schema::string())
            .min_items(1)
            .build()
            .unwrap();
        assert_eq!(
            schema.to_json(),
            r#"{"type":"array","items":{"type":"string"},"minItems":1}"#,
        );
    }

    #[test]
    fn prefix_items_accumulate_positionally() {
        let schema = Schema::array()
            .prefix_item(Schema::string())
            .prefix_items([Schema::integer(), Schema::integer()])
            .items(false)
            .build()
            .unwrap();

        let prefix = schema.as_object().unwrap().prefix_items.as_ref().unwrap();
        assert_eq!(prefix.len(), 3);
        assert_eq!(prefix[0], Schema::string().build().unwrap());
        assert_eq!(
            schema.as_object().unwrap().items.as_deref(),
            Some(&Schema::Bool(false))
        );
    }

    #[test]
    fn contains_carries_its_own_bounds() {
        let schema = Schema::array()
            .contains(Schema::builder().constant("admin"))
            .min_contains(1)
            .max_contains(1)
            .build()
            .unwrap();

        assert_eq!(
            schema.to_json(),
            r#"{"type":"array","contains":{"const":"admin"},"maxContains":1,"minContains":1}"#,
        );
    }

    #[test]
    fn unevaluated_items_renders_after_the_array_assertions() {
        let schema = Schema::array()
            .unique_items(true)
            .unevaluated_items(false)
            .build()
            .unwrap();
        assert_eq!(
            schema.to_json(),
            r#"{"type":"array","uniqueItems":true,"unevaluatedItems":false}"#,
        );
    }
}

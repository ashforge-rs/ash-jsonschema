//! Keywords that only mean something for a number instance.
//!
//! `number` and `integer` share this vocabulary exactly — the two differ only
//! in the `type` keyword [`Schema::number`](crate::Schema::number) and
//! [`Schema::integer`](crate::Schema::integer) commit to — so both use
//! [`NumericBuilder`](crate::NumericBuilder).

use crate::builder::{Builder, kind};
use crate::schema::Num;

impl Builder<kind::Numeric> {
    /// Sets `minimum`, an inclusive lower bound.
    pub fn minimum(mut self, minimum: impl Into<Num>) -> Self {
        self.inner.minimum = Some(minimum.into());
        self
    }

    /// Sets `maximum`, an inclusive upper bound.
    pub fn maximum(mut self, maximum: impl Into<Num>) -> Self {
        self.inner.maximum = Some(maximum.into());
        self
    }

    /// Sets `exclusiveMinimum`.
    ///
    /// In 2020-12 this is a number in its own right, not the boolean modifier
    /// of `minimum` that older drafts used.
    pub fn exclusive_minimum(mut self, minimum: impl Into<Num>) -> Self {
        self.inner.exclusive_minimum = Some(minimum.into());
        self
    }

    /// Sets `exclusiveMaximum`.
    pub fn exclusive_maximum(mut self, maximum: impl Into<Num>) -> Self {
        self.inner.exclusive_maximum = Some(maximum.into());
        self
    }

    /// Sets `minimum` and `maximum` together, both inclusive.
    ///
    /// ```
    /// # use ash_jsonschema_builder::Schema;
    /// let schema = Schema::integer().range(1, 10).build().unwrap();
    /// assert_eq!(schema.to_json(), r#"{"type":"integer","maximum":10,"minimum":1}"#);
    /// ```
    pub fn range(self, minimum: impl Into<Num>, maximum: impl Into<Num>) -> Self {
        self.minimum(minimum).maximum(maximum)
    }

    /// Sets `multipleOf`. The specification requires a value strictly greater
    /// than zero; nothing here checks that.
    pub fn multiple_of(mut self, divisor: impl Into<Num>) -> Self {
        self.inner.multiple_of = Some(divisor.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use crate::schema::Schema;

    #[test]
    fn numeric_assertions_render_with_their_specified_names() {
        let schema = Schema::number()
            .multiple_of(0.5)
            .minimum(-1)
            .maximum(1)
            .build()
            .unwrap();

        assert_eq!(
            schema.to_json(),
            r#"{"type":"number","multipleOf":0.5,"maximum":1,"minimum":-1}"#,
        );
    }

    #[test]
    fn exclusive_bounds_are_numbers_not_booleans() {
        let schema = Schema::number()
            .exclusive_minimum(0)
            .exclusive_maximum(1)
            .build()
            .unwrap();
        assert_eq!(
            schema.to_json(),
            r#"{"type":"number","exclusiveMaximum":1,"exclusiveMinimum":0}"#,
        );
    }

    #[test]
    fn integer_and_number_share_the_vocabulary_and_differ_only_in_type() {
        let integer = Schema::integer().minimum(0).build().unwrap();
        let number = Schema::number().minimum(0).build().unwrap();

        assert_ne!(integer, number);
        assert_eq!(
            integer.as_object().unwrap().minimum,
            number.as_object().unwrap().minimum
        );
    }
}

//! Keywords that only mean something for a string instance: the string
//! assertions, `format`, and the content vocabulary.

use crate::builder::{Builder, kind};
use crate::format::Format;
use crate::schema::Schema;

impl Builder<kind::Str> {
    /// Sets `minLength`, counted in Unicode code points rather than bytes.
    pub fn min_length(mut self, length: u64) -> Self {
        self.inner.min_length = Some(length);
        self
    }

    /// Sets `maxLength`, counted in Unicode code points rather than bytes.
    pub fn max_length(mut self, length: u64) -> Self {
        self.inner.max_length = Some(length);
        self
    }

    /// Sets `minLength` and `maxLength` to the same value.
    pub fn length(self, length: u64) -> Self {
        self.min_length(length).max_length(length)
    }

    /// Sets `pattern`, an ECMA-262 regular expression.
    ///
    /// The expression is **unanchored**: it asserts that a match exists
    /// somewhere in the string, so anchor it with `^` and `$` yourself when you
    /// mean the whole string.
    pub fn pattern(mut self, pattern: impl Into<String>) -> Self {
        self.inner.pattern = Some(pattern.into());
        self
    }

    /// Sets `format`. Takes a [`Format`] or any string — an unrecognised name
    /// becomes [`Format::Custom`].
    ///
    /// ```
    /// # use ash_jsonschema_builder::{Format, Schema};
    /// assert_eq!(
    ///     Schema::string().format(Format::DateTime).build().unwrap(),
    ///     Schema::string().format("date-time").build().unwrap(),
    /// );
    /// ```
    pub fn format(mut self, format: impl Into<Format>) -> Self {
        self.inner.format = Some(format.into());
        self
    }

    /// Sets `contentEncoding`, e.g. `base64` — how the string decodes to
    /// octets.
    pub fn content_encoding(mut self, encoding: impl Into<String>) -> Self {
        self.inner.content_encoding = Some(encoding.into());
        self
    }

    /// Sets `contentMediaType`, the media type of the decoded string.
    pub fn content_media_type(mut self, media_type: impl Into<String>) -> Self {
        self.inner.content_media_type = Some(media_type.into());
        self
    }

    /// Sets `contentSchema`, a schema for the decoded content. Meaningful only
    /// alongside `contentMediaType`.
    pub fn content_schema(mut self, schema: impl Into<Schema>) -> Self {
        self.inner.content_schema = Some(Box::new(schema.into()));
        self
    }
}

#[cfg(test)]
mod tests {
    use crate::schema::Schema;

    #[test]
    fn string_assertions_render_with_their_specified_names() {
        let schema = Schema::string()
            .min_length(1)
            .max_length(64)
            .pattern("^[a-z]+$")
            .format("email")
            .build()
            .unwrap();

        assert_eq!(
            schema.to_json(),
            r#"{"type":"string","maxLength":64,"minLength":1,"pattern":"^[a-z]+$","format":"email"}"#,
        );
    }

    #[test]
    fn length_pins_both_bounds() {
        let schema = Schema::string().length(3).build().unwrap();
        assert_eq!(
            schema.to_json(),
            r#"{"type":"string","maxLength":3,"minLength":3}"#
        );
    }

    #[test]
    fn the_content_vocabulary_describes_an_embedded_document() {
        let schema = Schema::string()
            .content_encoding("base64")
            .content_media_type("application/json")
            .content_schema(Schema::object().required(["id"]))
            .build()
            .unwrap();

        assert_eq!(
            schema.to_json(),
            r#"{"type":"string","contentEncoding":"base64","contentMediaType":"application/json","contentSchema":{"type":"object","required":["id"]}}"#,
        );
    }
}

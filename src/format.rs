//! The `format` keyword's defined attributes.
//!
//! In 2020-12 `format` lives in its own vocabulary and is an *annotation* by
//! default: a validator may report it, but is not required to assert it. The
//! spec still defines a fixed set of attributes, and [`Format`] names them so a
//! typo becomes a compile error rather than a silently ignored keyword.
//!
//! Anything outside that set — a domain-specific attribute a private validator
//! understands — goes through [`Format::Custom`], which any `&str` converts
//! into automatically.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A `format` attribute.
///
/// ```
/// # use ash_jsonschema::Format;
/// assert_eq!(Format::DateTime.as_str(), "date-time");
/// assert_eq!(Format::from("email"), Format::Email);
/// assert_eq!(Format::from("snowflake"), Format::Custom("snowflake".into()));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Format {
    /// `date-time` — RFC 3339 timestamp.
    DateTime,
    /// `date` — RFC 3339 full-date.
    Date,
    /// `time` — RFC 3339 full-time.
    Time,
    /// `duration` — ISO 8601 duration.
    Duration,
    /// `email` — RFC 5321 mailbox.
    Email,
    /// `idn-email` — RFC 6531 internationalized mailbox.
    IdnEmail,
    /// `hostname` — RFC 1123 host name.
    Hostname,
    /// `idn-hostname` — RFC 5890 internationalized host name.
    IdnHostname,
    /// `ipv4` — dotted-quad address.
    Ipv4,
    /// `ipv6` — RFC 4291 address.
    Ipv6,
    /// `uri` — RFC 3986 URI.
    Uri,
    /// `uri-reference` — RFC 3986 URI reference.
    UriReference,
    /// `iri` — RFC 3987 IRI.
    Iri,
    /// `iri-reference` — RFC 3987 IRI reference.
    IriReference,
    /// `uuid` — RFC 4122 UUID.
    Uuid,
    /// `uri-template` — RFC 6570 template.
    UriTemplate,
    /// `json-pointer` — RFC 6901 pointer.
    JsonPointer,
    /// `relative-json-pointer` — relative JSON pointer.
    RelativeJsonPointer,
    /// `regex` — ECMA-262 regular expression.
    Regex,
    /// A format attribute outside the set the specification defines.
    Custom(String),
}

impl Format {
    /// The attribute as it is written in the schema.
    pub fn as_str(&self) -> &str {
        match self {
            Self::DateTime => "date-time",
            Self::Date => "date",
            Self::Time => "time",
            Self::Duration => "duration",
            Self::Email => "email",
            Self::IdnEmail => "idn-email",
            Self::Hostname => "hostname",
            Self::IdnHostname => "idn-hostname",
            Self::Ipv4 => "ipv4",
            Self::Ipv6 => "ipv6",
            Self::Uri => "uri",
            Self::UriReference => "uri-reference",
            Self::Iri => "iri",
            Self::IriReference => "iri-reference",
            Self::Uuid => "uuid",
            Self::UriTemplate => "uri-template",
            Self::JsonPointer => "json-pointer",
            Self::RelativeJsonPointer => "relative-json-pointer",
            Self::Regex => "regex",
            Self::Custom(other) => other,
        }
    }

    /// Whether this is one of the attributes the specification defines (as
    /// opposed to a [`Custom`](Format::Custom) one).
    pub fn is_standard(&self) -> bool {
        !matches!(self, Self::Custom(_))
    }
}

impl From<&str> for Format {
    /// Recognises the specified attributes by name; everything else becomes
    /// [`Format::Custom`].
    fn from(value: &str) -> Self {
        match value {
            "date-time" => Self::DateTime,
            "date" => Self::Date,
            "time" => Self::Time,
            "duration" => Self::Duration,
            "email" => Self::Email,
            "idn-email" => Self::IdnEmail,
            "hostname" => Self::Hostname,
            "idn-hostname" => Self::IdnHostname,
            "ipv4" => Self::Ipv4,
            "ipv6" => Self::Ipv6,
            "uri" => Self::Uri,
            "uri-reference" => Self::UriReference,
            "iri" => Self::Iri,
            "iri-reference" => Self::IriReference,
            "uuid" => Self::Uuid,
            "uri-template" => Self::UriTemplate,
            "json-pointer" => Self::JsonPointer,
            "relative-json-pointer" => Self::RelativeJsonPointer,
            "regex" => Self::Regex,
            other => Self::Custom(other.to_owned()),
        }
    }
}

impl From<String> for Format {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for Format {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Format {
    /// Reads the attribute back through [`From<&str>`](Format::from), so a name
    /// the specification defines round-trips to its own variant and anything
    /// else lands in [`Format::Custom`].
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from(String::deserialize(deserializer)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_specified_attribute() {
        let specified = [
            Format::DateTime,
            Format::Date,
            Format::Time,
            Format::Duration,
            Format::Email,
            Format::IdnEmail,
            Format::Hostname,
            Format::IdnHostname,
            Format::Ipv4,
            Format::Ipv6,
            Format::Uri,
            Format::UriReference,
            Format::Iri,
            Format::IriReference,
            Format::Uuid,
            Format::UriTemplate,
            Format::JsonPointer,
            Format::RelativeJsonPointer,
            Format::Regex,
        ];

        for format in specified {
            assert!(format.is_standard());
            assert_eq!(Format::from(format.as_str()), format);
        }
    }

    #[test]
    fn unknown_attributes_become_custom() {
        let format = Format::from("account-number");
        assert_eq!(format, Format::Custom("account-number".into()));
        assert!(!format.is_standard());
        assert_eq!(
            serde_json::to_string(&format).unwrap(),
            r#""account-number""#
        );
    }
}

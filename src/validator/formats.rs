//! Asserting `format`, and letting a caller decide what a name means.
//!
//! In 2020-12 `format` is an *annotation* by default: a validator may report it
//! and is not required to enforce it. That default is what this crate does
//! unless asked otherwise, because a schema whose `format` silently asserts
//! nothing on one validator and rejects data on another is a bad contract.
//!
//! Turning assertion on is therefore explicit, and per name:
//!
//! ```
//! # use ash_jsonschema::{Schema, Validator};
//! let schema = Schema::string().format("date").build()?;
//!
//! // Annotation by default: nothing is asserted.
//! assert!(Validator::new(&schema)?.is_valid(&serde_json::json!("not a date")));
//!
//! // Asserted on request.
//! let strict = Validator::builder(&schema).assert_formats().build()?;
//! assert!(!strict.is_valid(&serde_json::json!("not a date")));
//! assert!(strict.is_valid(&serde_json::json!("2024-02-29")));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## Your own names
//!
//! A `format` this crate does not know is one you can teach it, and the schema
//! stays ordinary JSON — another validator simply annotates where this one
//! asserts:
//!
//! ```
//! # use ash_jsonschema::{Schema, Validator};
//! let schema = Schema::string().format("employee-id").build()?;
//!
//! let validator = Validator::builder(&schema)
//!     .format("employee-id", |value| {
//!         value.strip_prefix("E-").is_some_and(|rest| {
//!             rest.len() == 5 && rest.chars().all(|c| c.is_ascii_digit())
//!         })
//!     })
//!     .build()?;
//!
//! assert!(validator.is_valid(&serde_json::json!("E-01234")));
//! assert!(!validator.is_valid(&serde_json::json!("E-1")));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Registering a handler asserts that name whether or not
//! [`assert_formats`](super::ValidatorBuilder::assert_formats) was called: you
//! only write a handler because you want it to run.
//!
//! ## What is checked
//!
//! A handler sees the string and answers whether it is well formed. Only string
//! instances reach one — the specification says `format` applies to strings and
//! is ignored for every other type.

use std::collections::HashMap;
use std::sync::Arc;

/// Decides whether a string is a well-formed instance of one `format`.
///
/// Shared, so a [`Validator`](crate::Validator) stays `Send + Sync` and cheap
/// to clone.
pub type FormatFn = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// The `format` names a validator will assert, and how.
#[derive(Clone, Default)]
pub(crate) struct Formats {
    /// Names registered by the caller, which always assert.
    custom: HashMap<String, FormatFn>,
    /// Whether the specification's own names assert too.
    assert_standard: bool,
}

impl std::fmt::Debug for Formats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Formats")
            .field("custom", &self.custom.keys().collect::<Vec<_>>())
            .field("assert_standard", &self.assert_standard)
            .finish()
    }
}

impl Formats {
    /// Registers a handler for `name`, replacing any already there.
    ///
    /// A name the specification defines may be overridden this way, which is
    /// how a stricter — or more lenient — reading of `email` or `hostname` gets
    /// used in place of the built-in one.
    pub(crate) fn insert(&mut self, name: impl Into<String>, handler: FormatFn) {
        self.custom.insert(name.into(), handler);
    }

    /// Turns on assertion for the names the specification defines.
    pub(crate) fn assert_standard(&mut self) {
        self.assert_standard = true;
    }

    /// Whether anything here asserts, which lets the evaluator skip the
    /// keyword entirely in the common case.
    pub(crate) fn is_inert(&self) -> bool {
        self.custom.is_empty() && !self.assert_standard
    }

    /// Checks `value` against `name`.
    ///
    /// `None` means nothing asserts that name — either it is unknown, or it is
    /// a standard one and standard assertion is off — which is the annotation
    /// behaviour the specification defaults to.
    pub(crate) fn check(&self, name: &str, value: &str) -> Option<bool> {
        if let Some(handler) = self.custom.get(name) {
            return Some(handler(value));
        }
        if self.assert_standard {
            return standard(name).map(|check| check(value));
        }
        None
    }
}

/// The check for a name the specification defines, if this crate implements it.
///
/// A standard name with no implementation here stays an annotation rather than
/// being reported as unknown: the specification permits a validator to assert
/// only the attributes it understands.
fn standard(name: &str) -> Option<fn(&str) -> bool> {
    Some(match name {
        "date" => is_date,
        "date-time" => is_date_time,
        "time" => is_time,
        "duration" => is_duration,
        "email" => is_email,
        "idn-email" => is_idn_email,
        "hostname" => is_hostname,
        "ipv4" => is_ipv4,
        "ipv6" => is_ipv6,
        "uuid" => is_uuid,
        "json-pointer" => is_json_pointer,
        "relative-json-pointer" => is_relative_json_pointer,
        _ => return None,
    })
}

// --- dates ----------------------------------------------------------------

/// Whether a year is a leap year in the proleptic Gregorian calendar.
fn is_leap_year(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// The number of days in a month, `1..=12`.
fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Parses exactly `width` ASCII digits, returning the value and the rest.
fn digits(text: &str, width: usize) -> Option<(u32, &str)> {
    let (head, rest) = text.split_at_checked(width)?;
    if head.len() != width || !head.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((head.parse().ok()?, rest))
}

/// RFC 3339 `full-date`: `YYYY-MM-DD`, and a day the month actually has.
///
/// ```text
/// 2024-02-29  ✓  2024 is a leap year
/// 2023-02-29  ✗  2023 is not
/// 2024-1-5    ✗  the fields are fixed width
/// ```
pub fn is_date(value: &str) -> bool {
    parse_date(value).is_some_and(|rest| rest.is_empty())
}

/// Parses a `full-date` prefix, returning what follows it.
fn parse_date(value: &str) -> Option<&str> {
    let (year, rest) = digits(value, 4)?;
    let rest = rest.strip_prefix('-')?;
    let (month, rest) = digits(rest, 2)?;
    let rest = rest.strip_prefix('-')?;
    let (day, rest) = digits(rest, 2)?;

    // A month outside 1..=12 gives no days, so the day check covers both.
    (day >= 1 && day <= days_in_month(year, month)).then_some(rest)
}

/// RFC 3339 `full-time`: `HH:MM:SS`, an optional fraction, and an offset.
///
/// A leap second — `:60` — is accepted, as the grammar permits.
pub fn is_time(value: &str) -> bool {
    parse_time(value).is_some_and(|rest| rest.is_empty())
}

/// Parses a `full-time` prefix, returning what follows it.
fn parse_time(value: &str) -> Option<&str> {
    let (hour, rest) = digits(value, 2)?;
    let rest = rest.strip_prefix(':')?;
    let (minute, rest) = digits(rest, 2)?;
    let rest = rest.strip_prefix(':')?;
    let (second, rest) = digits(rest, 2)?;

    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    // An optional fraction of a second: `.` and at least one digit.
    let rest = match rest.strip_prefix('.') {
        Some(fraction) => {
            let taken = fraction.bytes().take_while(u8::is_ascii_digit).count();
            if taken == 0 {
                return None;
            }
            &fraction[taken..]
        }
        None => rest,
    };

    // The offset is mandatory in RFC 3339: `Z`, or `±HH:MM`.
    let (offset_minutes, rest) = if let Some(rest) = rest.strip_prefix(['Z', 'z']) {
        (0i32, rest)
    } else {
        let negative = rest.starts_with('-');
        let rest = rest.strip_prefix(['+', '-'])?;
        let (offset_hour, rest) = digits(rest, 2)?;
        let rest = rest.strip_prefix(':')?;
        let (offset_minute, rest) = digits(rest, 2)?;
        if offset_hour > 23 || offset_minute > 59 {
            return None;
        }
        let magnitude = (offset_hour * 60 + offset_minute) as i32;
        (if negative { -magnitude } else { magnitude }, rest)
    };

    // A leap second is inserted at the end of a UTC day, so `:60` is only
    // legal at 23:59 *once the offset is taken off* — `23:59:60+01:00` is
    // 22:59:60 UTC, which is not a leap second.
    if second == 60 {
        let local = (hour * 60 + minute) as i32;
        let utc = local - offset_minutes;
        // Rust's `%` keeps the sign of the dividend; add a day first.
        if (utc + 1440) % 1440 != 23 * 60 + 59 {
            return None;
        }
    }

    Some(rest)
}

/// RFC 3339 `date-time`: a date, `T`, and a time.
pub fn is_date_time(value: &str) -> bool {
    let Some(rest) = parse_date(value) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix(['T', 't']) else {
        return false;
    };
    parse_time(rest).is_some_and(|rest| rest.is_empty())
}

/// ISO 8601 `duration`, as the specification's ABNF defines it.
///
/// ```text
/// P1Y2M3D    ✓   P1DT2H  ✓   PT0S  ✓   P1W  ✓
/// P          ✗   1Y      ✗   PT    ✗   P1S  ✗  (seconds need `T`)
/// ```
pub fn is_duration(value: &str) -> bool {
    let Some(rest) = value.strip_prefix('P') else {
        return false;
    };
    if rest.is_empty() {
        return false;
    }

    // The week form stands alone: `PnW`, and nothing else.
    if let Some(weeks) = rest.strip_suffix('W') {
        return !weeks.is_empty() && weeks.bytes().all(|byte| byte.is_ascii_digit());
    }

    let (date_part, time_part) = match rest.split_once('T') {
        Some((date, time)) => (date, Some(time)),
        None => (rest, None),
    };

    // `PT1H` has an empty date part, which is legal; `P` alone is not, and was
    // rejected above.
    if !date_part.is_empty() && !units_in_order(date_part, "YMD") {
        return false;
    }
    match time_part {
        // A `T` must be followed by at least one unit.
        Some(time) => !time.is_empty() && units_in_order(time, "HMS"),
        None => !date_part.is_empty(),
    }
}

/// Whether `text` is a run of `<digits><unit>` pairs drawn from `order`.
///
/// The ABNF nests each unit inside the one before it — `dur-date` is
/// `dur-day / dur-month / dur-year`, where `dur-year` is `1Y [dur-month]` —
/// so the units present must be **contiguous** in `order`, not merely in it.
/// `P1Y2M` and `P2M3D` are durations; `P1Y2D` is not, because it skips months.
fn units_in_order(text: &str, order: &str) -> bool {
    let mut remaining = text;
    let mut previous: Option<usize> = None;

    while !remaining.is_empty() {
        let taken = remaining.bytes().take_while(u8::is_ascii_digit).count();
        if taken == 0 {
            return false;
        }
        let (_, rest) = remaining.split_at(taken);
        let Some(unit) = rest.chars().next() else {
            // Digits with no unit after them.
            return false;
        };
        let Some(position) = order.find(unit) else {
            return false;
        };
        // Each unit must follow immediately on the one before it.
        if let Some(previous) = previous
            && position != previous + 1
        {
            return false;
        }
        previous = Some(position);
        remaining = &rest[unit.len_utf8()..];
    }
    true
}

// --- hosts and addresses --------------------------------------------------

/// A hostname label: alphanumerics and hyphens, not starting or ending with a
/// hyphen, at most 63 characters.
fn is_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

/// RFC 1123 hostname.
///
/// A single trailing dot is accepted — it makes a name fully qualified — and
/// the whole name is capped at 253 characters.
///
/// An `xn--` label is checked for shape but **not decoded**: confirming that it
/// encodes a valid internationalised name needs the IDNA and punycode rules,
/// which this crate does not implement. A malformed `xn--` label therefore
/// passes here. [`tld`](crate::validator::tld) applies the same limit.
pub fn is_hostname(value: &str) -> bool {
    // A single trailing dot makes the name fully qualified; it still needs a
    // name in front of it, so `example.` is one and `.` is not.
    let value = value.strip_suffix('.').unwrap_or(value);
    !value.is_empty() && value.len() <= 253 && value.split('.').all(is_label)
}

/// A dotted-quad IPv4 address, with no leading zeros.
pub fn is_ipv4(value: &str) -> bool {
    let mut parts = 0;
    for part in value.split('.') {
        parts += 1;
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        // `01` is not a legal octet: leading zeros invite octal confusion.
        if part.len() > 1 && part.starts_with('0') {
            return false;
        }
        if part.parse::<u32>().is_ok_and(|octet| octet > 255) {
            return false;
        }
    }
    parts == 4
}

/// An RFC 4291 IPv6 address, including the `::` compressed form and a trailing
/// dotted-quad.
pub fn is_ipv6(value: &str) -> bool {
    // At most one `::`, which stands for one or more groups of zeros.
    let (head, tail) = match value.split_once("::") {
        Some((head, tail)) => {
            if tail.contains("::") {
                return false;
            }
            (head, Some(tail))
        }
        None => (value, None),
    };

    /// Counts the 16-bit groups in one side, or `None` if any is malformed.
    /// A trailing IPv4 address stands for the final two groups.
    fn groups(text: &str, allow_v4: bool) -> Option<usize> {
        if text.is_empty() {
            return Some(0);
        }
        let parts: Vec<&str> = text.split(':').collect();
        let mut count = 0;
        for (index, part) in parts.iter().enumerate() {
            let last = index + 1 == parts.len();
            if last && allow_v4 && part.contains('.') {
                if !is_ipv4(part) {
                    return None;
                }
                count += 2;
                continue;
            }
            if part.is_empty()
                || part.len() > 4
                || !part.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return None;
            }
            count += 1;
        }
        Some(count)
    }

    match tail {
        // Compressed: the two sides together must leave room for the zeros.
        Some(tail) => {
            let Some(before) = groups(head, false) else {
                return false;
            };
            let Some(after) = groups(tail, true) else {
                return false;
            };
            before + after < 8
        }
        // Uncompressed: exactly eight groups.
        None => groups(head, true) == Some(8),
    }
}

// --- other ----------------------------------------------------------------

/// An RFC 5321 mailbox: a dot-atom local part, one `@`, and a domain.
///
/// **The quoted-string form is not accepted.** RFC 5321 permits a local part in
/// quotes — `"joe bloggs"@example.com` — which legalises spaces, consecutive
/// dots, and escapes that the unquoted grammar forbids. Parsing it properly
/// means implementing the quoting rules, and almost nothing deliverable uses
/// it, so this rejects the form rather than half-parsing it. Register your own
/// `email` handler if you need it.
///
/// For the internationalised form, see [`is_idn_email`].
pub fn is_email(value: &str) -> bool {
    // Exactly one `@`: `a@b@c.org` is not an address, so splitting on the last
    // one would wrongly accept it.
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    if domain.contains('@') {
        return false;
    }
    if local.is_empty() || local.len() > 64 {
        return false;
    }
    // A dot may separate parts of the local part but not begin, end, or double.
    if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return false;
    }
    // The dot-atom grammar is ASCII, and excludes the specials that only the
    // quoted form permits.
    if !local
        .chars()
        .all(|character| character.is_ascii_graphic() && !r#"()<>[]:;@\,""#.contains(character))
    {
        return false;
    }
    // A bracketed address literal is legal in place of a domain name.
    if let Some(literal) = domain.strip_prefix('[').and_then(|d| d.strip_suffix(']')) {
        let literal = literal.strip_prefix("IPv6:").unwrap_or(literal);
        return is_ipv4(literal) || is_ipv6(literal);
    }
    // A trailing dot is legal in a hostname but not in a mailbox domain.
    !domain.ends_with('.') && is_hostname(domain) && domain.contains('.')
}

/// An RFC 6531 internationalised mailbox.
///
/// The same structure as [`is_email`], with the local part permitted to carry
/// non-ASCII characters — which is the whole point of the format. The ASCII
/// specials that only a quoted local part may contain are still rejected, and
/// the quoted form itself is still not accepted.
pub fn is_idn_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    if domain.contains('@') || local.is_empty() {
        return false;
    }
    if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return false;
    }
    // Non-ASCII is what distinguishes this from `email`; the specials that
    // need quoting are still out.
    if local.chars().any(|character| {
        character.is_control()
            || character == ' '
            || (character.is_ascii() && r#"()<>[]:;@\,""#.contains(character))
    }) {
        return false;
    }
    if let Some(literal) = domain.strip_prefix('[').and_then(|d| d.strip_suffix(']')) {
        let literal = literal.strip_prefix("IPv6:").unwrap_or(literal);
        return is_ipv4(literal) || is_ipv6(literal);
    }
    !domain.ends_with('.') && is_hostname(domain) && domain.contains('.')
}

/// An RFC 4122 UUID: 8-4-4-4-12 hexadecimal digits.
pub fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    if groups.len() != 5 {
        return false;
    }
    [8, 4, 4, 4, 12].iter().zip(&groups).all(|(width, group)| {
        group.len() == *width && group.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

/// An RFC 6901 JSON Pointer: empty, or `/`-prefixed tokens whose only `~` is
/// part of `~0` or `~1`.
pub fn is_json_pointer(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    if !value.starts_with('/') {
        return false;
    }
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character == '~' && !matches!(characters.next(), Some('0' | '1')) {
            return false;
        }
    }
    true
}

/// A relative JSON Pointer: a non-negative integer, then either `#` or a JSON
/// Pointer.
pub fn is_relative_json_pointer(value: &str) -> bool {
    let digits = value.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return false;
    }
    // No leading zeros, except the number zero itself.
    if digits > 1 && value.starts_with('0') {
        return false;
    }
    let rest = &value[digits..];
    rest == "#" || is_json_pointer(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_respect_the_length_of_each_month() {
        assert!(is_date("2024-02-29")); // a leap year
        assert!(!is_date("2023-02-29")); // not one
        assert!(is_date("2000-02-29")); // divisible by 400
        assert!(!is_date("1900-02-29")); // divisible by 100, not 400
        assert!(is_date("2024-01-31"));
        assert!(!is_date("2024-04-31")); // April has 30
        assert!(!is_date("2024-13-01")); // no such month
        assert!(!is_date("2024-00-10"));
        assert!(!is_date("2024-01-00"));
    }

    #[test]
    fn a_date_must_be_written_in_the_fixed_width_form() {
        assert!(!is_date("2024-1-5"));
        assert!(!is_date("24-01-05"));
        assert!(!is_date("2024/01/05"));
        assert!(!is_date("2024-01-05T00:00:00Z")); // that is a date-time
        assert!(!is_date(""));
    }

    #[test]
    fn times_need_an_offset_and_accept_a_leap_second() {
        assert!(is_time("12:30:00Z"));
        assert!(is_time("12:30:00z"));
        assert!(is_time("12:30:00.123Z"));
        assert!(is_time("12:30:00+01:00"));
        assert!(is_time("12:30:00-05:30"));
        assert!(is_time("23:59:60Z")); // a leap second

        assert!(!is_time("12:30:00")); // no offset
        assert!(!is_time("24:00:00Z"));
        assert!(!is_time("12:60:00Z"));
        assert!(!is_time("12:30:61Z"));
        assert!(!is_time("12:30:00.Z")); // a fraction needs digits
        assert!(!is_time("12:30:00+24:00"));
    }

    #[test]
    fn a_date_time_is_a_date_then_a_time() {
        assert!(is_date_time("2024-02-29T12:30:00Z"));
        assert!(is_date_time("2024-02-29t12:30:00.5+02:00"));

        assert!(!is_date_time("2023-02-29T12:30:00Z")); // the date is wrong
        assert!(!is_date_time("2024-02-29 12:30:00Z")); // a space, not `T`
        assert!(!is_date_time("2024-02-29")); // no time at all
    }

    #[test]
    fn durations_follow_the_unit_order() {
        assert!(is_duration("P1Y"));
        assert!(is_duration("P1Y2M3D"));
        assert!(is_duration("P1DT2H"));
        assert!(is_duration("PT0S"));
        assert!(is_duration("PT1H30M"));
        assert!(is_duration("P3W"));

        assert!(!is_duration("P")); // nothing at all
        assert!(!is_duration("1Y")); // no `P`
        assert!(!is_duration("PT")); // a `T` with no unit
        assert!(!is_duration("P1S")); // seconds belong after `T`
        assert!(!is_duration("P2M1Y")); // out of order
        assert!(!is_duration("P1Y1Y")); // repeated
        assert!(!is_duration("P1W2D")); // the week form stands alone
    }

    #[test]
    fn a_leap_second_is_only_legal_at_the_end_of_a_utc_day() {
        // 23:59:60Z is the inserted second; the same wall clock at another
        // offset is a different UTC instant and not one.
        assert!(is_time("23:59:60Z"));
        assert!(is_time("23:59:60+00:00"));
        assert!(is_time("00:59:60+01:00")); // 23:59:60 UTC
        assert!(is_time("22:59:60-01:00")); // also 23:59:60 UTC

        assert!(!is_time("22:59:60Z"));
        assert!(!is_time("23:58:60Z"));
        assert!(!is_time("23:59:60+01:00")); // 22:59:60 UTC
        assert!(!is_date_time("1998-12-31T23:58:60Z"));
        assert!(is_date_time("1998-12-31T23:59:60Z"));
    }

    #[test]
    fn duration_units_must_be_contiguous_not_merely_ordered() {
        // The ABNF nests each unit in the one before it, so a gap is not a
        // duration even though the units that are present are in order.
        assert!(is_duration("P1Y2M"));
        assert!(is_duration("P2M3D"));
        assert!(is_duration("P1Y2M3D"));
        assert!(is_duration("PT1H2M"));

        assert!(!is_duration("P1Y2D")); // months skipped
        assert!(!is_duration("PT1H2S")); // minutes skipped
        assert!(!is_duration("P2D1Y")); // out of order
    }

    #[test]
    fn an_address_has_exactly_one_at_sign() {
        assert!(is_email("someone@example.com"));
        assert!(!is_email("a@b@c.org"));
        assert!(!is_email("test\\@test@iana.org"));
    }

    #[test]
    fn the_quoted_local_part_form_is_not_accepted() {
        // Legal RFC 5321, and deliberately out of scope — see `is_email`.
        assert!(!is_email(r#""joe bloggs"@example.com"#));
        assert!(!is_email(r#"" "@iana.org"#));
        // The specials that only quoting permits are rejected unquoted too.
        assert!(!is_email("test\"@iana.org"));
        assert!(!is_email("a b@example.com"));
    }

    #[test]
    fn only_the_internationalised_form_takes_non_ascii() {
        assert!(!is_email("aé@iana.org"));
        assert!(is_idn_email("aé@iana.org"));
        assert!(is_idn_email("θσερ@example.com"));

        // Structure is still enforced on both.
        assert!(!is_idn_email("é@iana.org."));
        assert!(!is_idn_email("a@b@c.org"));
        assert!(!is_idn_email("a b@example.com"));
    }

    #[test]
    fn a_mailbox_domain_takes_no_trailing_dot() {
        // Legal in a hostname, not in an address.
        assert!(is_hostname("iana.org."));
        assert!(!is_email("test@iana.org."));
    }

    #[test]
    fn hostnames_reject_malformed_labels() {
        assert!(is_hostname("example.com"));
        assert!(is_hostname("example.com.")); // fully qualified
        assert!(is_hostname("a-b.example"));
        assert!(is_hostname("localhost"));

        assert!(!is_hostname("-leading.example"));
        assert!(!is_hostname("trailing-.example"));
        assert!(!is_hostname("double..dot"));
        assert!(!is_hostname("under_score.example"));
        assert!(!is_hostname(""));
        assert!(!is_hostname(&format!("{}.example", "a".repeat(64))));
    }

    #[test]
    fn ipv4_rejects_leading_zeros_and_wrong_arity() {
        assert!(is_ipv4("192.168.0.1"));
        assert!(is_ipv4("0.0.0.0"));
        assert!(is_ipv4("255.255.255.255"));

        assert!(!is_ipv4("192.168.0"));
        assert!(!is_ipv4("192.168.0.1.2"));
        assert!(!is_ipv4("256.0.0.1"));
        assert!(!is_ipv4("01.2.3.4")); // a leading zero
        assert!(!is_ipv4("1.2.3.a"));
    }

    #[test]
    fn ipv6_handles_compression_and_embedded_ipv4() {
        assert!(is_ipv6("2001:0db8:0000:0000:0000:ff00:0042:8329"));
        assert!(is_ipv6("2001:db8::ff00:42:8329"));
        assert!(is_ipv6("::1"));
        assert!(is_ipv6("::"));
        assert!(is_ipv6("::ffff:192.168.0.1"));

        assert!(!is_ipv6("2001:db8::ff00::1")); // two `::`
        assert!(!is_ipv6("2001:db8:0:0:0:ff00:42")); // seven groups
        assert!(!is_ipv6("2001:db8:0:0:0:0:ff00:42:8329")); // nine
        assert!(!is_ipv6("gggg::1"));
        assert!(!is_ipv6("2001:db8::192.168.0.256"));
    }

    #[test]
    fn emails_are_checked_structurally() {
        assert!(is_email("someone@example.com"));
        assert!(is_email("first.last@sub.example.com"));
        assert!(is_email("a+b@example.com"));
        assert!(is_email("user@[192.168.0.1]"));

        assert!(!is_email("no-at-sign"));
        assert!(!is_email("@example.com"));
        assert!(!is_email("someone@"));
        assert!(!is_email(".lead@example.com"));
        assert!(!is_email("double..dot@example.com"));
        assert!(!is_email("someone@localhost")); // a domain needs a dot
    }

    #[test]
    fn uuids_are_grouped_hex() {
        assert!(is_uuid("8f14e45f-ceea-467a-9f8a-1b2c3d4e5f60"));
        assert!(is_uuid("00000000-0000-0000-0000-000000000000"));

        assert!(!is_uuid("8f14e45f-ceea-467a-9f8a-1b2c3d4e5f6")); // short
        assert!(!is_uuid("8f14e45fceea467a9f8a1b2c3d4e5f60")); // no dashes
        assert!(!is_uuid("8f14e45f-ceea-467a-9f8a-1b2c3d4e5f6g")); // not hex
    }

    #[test]
    fn json_pointers_check_their_escapes() {
        assert!(is_json_pointer(""));
        assert!(is_json_pointer("/"));
        assert!(is_json_pointer("/a/b"));
        assert!(is_json_pointer("/a~0b/c~1d"));

        assert!(!is_json_pointer("a/b")); // no leading slash
        assert!(!is_json_pointer("/a~2b")); // not an escape
        assert!(!is_json_pointer("/a~")); // dangling
    }

    #[test]
    fn relative_json_pointers_lead_with_a_count() {
        assert!(is_relative_json_pointer("0"));
        assert!(is_relative_json_pointer("1/a"));
        assert!(is_relative_json_pointer("0#"));

        assert!(!is_relative_json_pointer("/a")); // no count
        assert!(!is_relative_json_pointer("01/a")); // a leading zero
        assert!(!is_relative_json_pointer(""));
    }

    #[test]
    fn nothing_asserts_until_something_is_registered() {
        let mut formats = Formats::default();
        assert!(formats.is_inert());
        assert_eq!(formats.check("date", "nonsense"), None);

        formats.assert_standard();
        assert!(!formats.is_inert());
        assert_eq!(formats.check("date", "nonsense"), Some(false));
        assert_eq!(formats.check("date", "2024-01-01"), Some(true));
        // A standard name this crate does not implement stays an annotation.
        assert_eq!(formats.check("iri", "anything"), None);
    }

    #[test]
    fn a_registered_handler_asserts_without_the_standard_switch() {
        let mut formats = Formats::default();
        formats.insert("even-length", Arc::new(|value: &str| value.len() % 2 == 0));

        assert!(!formats.is_inert());
        assert_eq!(formats.check("even-length", "ab"), Some(true));
        assert_eq!(formats.check("even-length", "abc"), Some(false));
        // Standard names are still inert: only what was registered asserts.
        assert_eq!(formats.check("date", "nonsense"), None);
    }

    #[test]
    fn a_handler_overrides_the_built_in_of_the_same_name() {
        let mut formats = Formats::default();
        formats.assert_standard();
        assert_eq!(formats.check("date", "whenever"), Some(false));

        formats.insert("date", Arc::new(|_: &str| true));
        assert_eq!(formats.check("date", "whenever"), Some(true));
    }
}

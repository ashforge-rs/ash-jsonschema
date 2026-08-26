# Changelog

All notable changes to this crate are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this crate adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] — 2026-08-26

First release. A typed builder for JSON Schema 2020-12, plus the machinery for
treating a schema as a document: reading it, resolving its references,
comparing versions, bundling, and checking instances against it.

### Building

- Typed builders per JSON type — `Schema::string()`, `object()`, `array()`,
  `integer()`, `number()`, `boolean()`, `null()` — where each type's vocabulary
  is the only one in scope. `Schema::integer().min_length(3)` does not compile.
- `.build()` checks the result against the embedded 2020-12 meta-schema, so a
  schema that exists is one that is well formed. Errors carry a JSON Pointer to
  the keyword at fault.
- `.build_frozen()` for a schema that is done, and `export`/`export_at` for a
  detached copy of a whole schema or one subtree.
- `.keyword()` writes extension vocabularies, and panics on a name the dialect
  defines rather than emitting a duplicate JSON key.
- `object!`, `defs!`, `one_of_tagged!`, and `validator!` for shapes that repeat
  structure. Each produces byte-identical output to the chain it stands for.

### Reading

- `Schema::parse` and `parse_checked`, with `Deserialize` across the whole tree.
  Property order survives the round trip.
- Unknown keywords are kept, not dropped: `x-internal` and `discriminator`
  round-trip verbatim through `SchemaObject::extensions`.
- A parsed schema resumes as a builder via `into_builder` / `into_typed`.
- Reading without pattern-matching: `property`, `requires`, `def`, `pointer`,
  `schema_type`.

### Resolving

- `Registry` indexes schemas under their `$id` and resolves `$ref` by pointer or
  `$anchor`, across documents and through nested `$id`s. Offline — nothing is
  fetched.
- `$dynamicAnchor` indexed with the outermost-wins rule recursive schemas need.
- `check_references` reports each dangling `$ref` at its own pointer, opt-in
  because a dangling `$ref` is a legal 2020-12 document.
- `bundle` embeds each referenced document under `$defs` keeping its `$id`, so
  no `$ref` is rewritten.

### Comparing

- `diff` and `diff_resolved` classify each change by audience: a change breaks
  writers and readers in opposite directions, so a `Change` carries both
  effects. What cannot be classified is `Unknown`, and counts as breaking.

### Validating

*Behind the `validation` feature, on by default.*

- `Validator` with a schema fixed for its life. `Mode::All` reports every error;
  `Mode::FailFast` stops at the first.
- Every assertion and applicator of the dialect, including
  `unevaluatedProperties` and `unevaluatedItems` with the annotation collection
  they require, and `$dynamicRef` against the runtime dynamic scope.
- Results are the specification's output units: `instance_location`,
  `keyword_location`, `absolute_keyword_location`, `error`, `annotation`. All
  four output formats — `Flag`, `Basic`, `Detailed`, `Verbose` — via
  `Outcome::output`, validating against the official output schema.
- Errors nest, so a failed `anyOf` keeps why each alternative was rejected
  rather than reporting only that none matched. `Outcome::errors` is the flat
  view of the same run.
- `Validator::annotate` collects annotations; `validate` does not.
- `format` is an annotation by default, as 2020-12 specifies.
  `assert_formats()` turns on the built-in checks and `.format(name, check)`
  registers your own. Dates are checked against the calendar, not just the
  shape.
- `validator::tld` answers whether a suffix is *shaped* like a TLD (always) or
  *is* one (behind the `iana-tlds` feature, which vendors the IANA root zone).

### Conformance

Against the official
[JSON-Schema-Test-Suite](https://github.com/json-schema-org/JSON-Schema-Test-Suite):

| Suite | Result |
| --- | --- |
| draft2020-12 | 1281 / 1299 (98.6%) |
| output tests | 4 / 4 |

Known gaps, all documented in the README: custom `$vocabulary` declarations that
disable keywords, `$dynamicRef` across several intermediate scopes, patterns
needing Unicode property escapes (`regex-lite` does not implement them), and —
in the optional `format` tests — `xn--` punycode decoding and quoted email
local parts, both of which need full IDNA rules.

[0.1.0]: https://github.com/ashforge-rs/ash-jsonschema/releases/tag/v0.1.0

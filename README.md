# ash-jsonschema

[![crates.io](https://img.shields.io/crates/v/ash-jsonschema.svg)](https://crates.io/crates/ash-jsonschema)
[![docs.rs](https://docs.rs/ash-jsonschema/badge.svg)](https://docs.rs/ash-jsonschema)
[![CI](https://github.com/ashforge-rs/ash-jsonschema/actions/workflows/ci.yml/badge.svg)](https://github.com/ashforge-rs/ash-jsonschema/actions/workflows/ci.yml)

A typed builder for [JSON Schema 2020-12](https://json-schema.org/draft/2020-12/release-notes):
chain the keywords, call `.build()`, get a schema. The crate also reads,
resolves, compares, bundles and validates against schemas.

> **Disclaimer:** this repository contains AI-generated code.

It supports one dialect only. 2020-12 is the only thing it emits, so there is
no draft selector to thread through the API.

## Installation

```sh
cargo add ash-jsonschema
```

## Quick start

```rust
use ash_jsonschema::{Format, Schema, SchemaError};

fn main() -> Result<(), SchemaError> {
    let schema = Schema::object()
        .dialect()
        .id("https://example.com/schemas/user")
        .title("User")
        .required_property("id", Schema::string().format(Format::Uuid))
        .required_property("email", Schema::string().format(Format::Email))
        .property("age", Schema::integer().minimum(0).nullable())
        .property("tags", Schema::array().items(Schema::string()).unique_items(true))
        .additional_properties(false)
        .build()?; // checked against the embedded 2020-12 meta-schema

    println!("{}", schema.to_json_pretty());
    Ok(())
}
```

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://example.com/schemas/user",
  "title": "User",
  "type": "object",
  "properties": {
    "id": { "type": "string", "format": "uuid" },
    "email": { "type": "string", "format": "email" },
    "age": { "type": ["integer", "null"], "minimum": 0 },
    "tags": { "type": "array", "items": { "type": "string" }, "uniqueItems": true }
  },
  "required": ["id", "email"],
  "additionalProperties": false
}
```

## Building schemas

- **Typed builders.** `Schema::string()`, `integer()`, `object()`, `array()`
  and the others each return a builder that has already set `type`. A
  type-specific keyword is only available on the builder it applies to, so
  `Schema::integer().min_length(3)` doesn't compile.
- **Only the outermost chain needs `.build()`.** Every keyword that takes a
  subschema accepts `impl Into<Schema>`, which builders and plain `bool`s
  satisfy.
- **`.build()` checks its work.** The 2020-12 meta-schema is embedded in the
  crate, so nothing is fetched at runtime. Every violation is reported with a
  JSON Pointer to the offending keyword. `build_unchecked()` skips the check.
- **`.build_frozen()`** returns a `FrozenSchema`: a cheaply cloned,
  `Send + Sync`, read-only handle for schemas that are finished.
- **Macros** for repetitive shapes: `object!` writes `properties` and
  `required` from one list, and `defs!` and `one_of_tagged!` cover definitions
  and tagged unions. Each produces exactly the same output as the builder
  chain it replaces.
- **Deterministic output.** Keywords and properties render in a stable order,
  so a regenerated schema diffs cleanly.

## Working with existing schemas

- **Parsing.** `Schema::parse()` reads a document, and `parse_checked()` also
  validates it. Keywords outside the dialect (`x-` annotations, OpenAPI's
  `discriminator`) are kept, so a parsed document renders back unchanged.
- **References.** A `Registry` holds schemas under their `$id` and resolves
  `$ref`, `$anchor` and `$dynamicRef` across documents.
  `check_references()` reports references that point nowhere. Nothing is ever
  fetched over the network.
- **Diffing.** `diff()` compares two versions of a schema and classifies each
  change separately for writers and readers of the data. For example, adding a
  required property breaks writers but not readers. Changes it can't classify
  count as breaking.
- **Bundling.** `bundle()` embeds every referenced document under `$defs`, so
  the result resolves on its own.

## Validating instances

The `validation` feature (on by default) adds `Validator`:

```rust
use ash_jsonschema::{Schema, object, validator};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let order = validator! {
        object! {
            required "id": Schema::string().min_length(1),
            required "total": Schema::integer().minimum(0),
        }
        .closed()
    }?;

    assert!(order.is_valid(&json!({"id": "a1", "total": 250})));
    assert!(!order.is_valid(&json!({"id": "", "total": -1})));
    Ok(())
}
```

- Errors carry the instance location, the keyword location and, once a `$ref`
  has been crossed, the absolute keyword location.
- `Mode::All` reports every error and `Mode::FailFast` stops at the first.
- Results render in all four standard output formats (`Flag`, `Basic`,
  `Detailed`, `Verbose`).
- `format` is an annotation by default, as the specification says. Call
  `assert_formats()` to enforce the built-in formats, or register your own
  handlers.
- The validator passes 1281 of the 1299 cases (98.6%) in the official
  [JSON-Schema-Test-Suite](https://github.com/json-schema-org/JSON-Schema-Test-Suite)
  for draft 2020-12. The failures are custom `$vocabulary` declarations, some
  multi-scope `$dynamicRef` cases, and patterns that need Unicode property
  escapes, which `regex-lite` doesn't support.

## Cargo features

| Feature | Default | Enables |
|---|---|---|
| `validation` | yes | `Validator`, `Outcome`, the `validator!` macro and `format` handlers |
| `iana-tlds` | no | A vendored snapshot of the IANA root zone, for checking that a TLD exists |

Building, reading, resolving, diffing and bundling all work with
`default-features = false`.

## Limitations

- No `$ref` is ever retrieved over the network, and schemas aren't generated
  from Rust types.
- The meta-schema check validates a schema's shape, not whether it makes sense.
  A `minLength` above its `maxLength` is a legal document.
- `pattern` uses `regex-lite`, so lookahead, backreferences and Unicode
  property escapes aren't supported. Such patterns are rejected when the
  validator is built.

## Minimum supported Rust version

Rust 1.88 or later.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). To report a security issue, see
[SECURITY.md](SECURITY.md).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

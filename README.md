# ash-jsonschema-builder

**Chain the keywords, call `.build()`, get a schema.**

A typed builder for [JSON Schema 2020-12](https://json-schema.org/draft/2020-12/release-notes).
One dialect, deliberately — the latest standard is the only thing this crate
emits, so there is no draft selector to thread through the API and no keyword
that means one thing here and another there.

```rust
use ash_jsonschema_builder::{Format, Schema};

let schema = Schema::object()
    .dialect()
    .id("https://example.com/schemas/user")
    .title("User")
    .required_property("id", Schema::string().format(Format::Uuid))
    .required_property("email", Schema::string().format(Format::Email))
    .property("age", Schema::integer().minimum(0).nullable())
    .property("tags", Schema::array().items(Schema::string()).unique_items(true))
    .additional_properties(false)
    .build()?;   // checked against the embedded 2020-12 meta-schema

println!("{}", schema.to_json_pretty());
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

## The type is not decoration

`Schema` is both the output type and the entry point. Each constructor returns a
builder that has already committed the `type` keyword:

| Entry point | `type` | Builder |
|---|---|---|
| `Schema::string()` | `string` | `StringBuilder` |
| `Schema::number()` / `Schema::integer()` | `number` / `integer` | `NumericBuilder` |
| `Schema::object()` | `object` | `ObjectBuilder` |
| `Schema::array()` | `array` | `ArrayBuilder` |
| `Schema::boolean()` / `Schema::null()` | `boolean` / `null` | `AnyBuilder` |
| `Schema::builder()`, `Schema::all_of()`, `Schema::reference()`, … | *(none)* | `AnyBuilder` |

The core, metadata, and applicator vocabularies are available on every builder,
but a type-specific assertion is reachable only from the builder it applies to.
`.min_length()` exists on a string schema and nowhere else, so a keyword a
validator would silently ignore does not compile in the first place:

```rust
Schema::integer().min_length(3);   // error: no method named `min_length`
```

## Only the outermost chain needs `.build()`

Every keyword that takes a subschema accepts `impl Into<Schema>`, which a
builder satisfies directly — and so does a bare `bool`, since `true` and `false`
are themselves schemas:

```rust
let schema = Schema::object()
    .property("nested", Schema::object().additional_properties(false))
    .property("anything", true)
    .build()?;
```

## `.build()` checks its work

All eight documents of the 2020-12 meta-schema are vendored into the crate and
parsed once into memory on first use. Nothing is fetched at runtime, so a build
is reproducible and works offline. `.build()` runs the document you just
assembled against them and hands back a `SchemaError` instead of a `Schema` if
it does not hold up.

The check is a depth-first walk: the root and every subschema beneath it are
validated in turn, each in its own right. A nested builder needs no `.build()`
of its own and is checked all the same, located by JSON Pointer:

```rust
let error = Schema::object()
    .property("inner", Schema::string().anchor("not a valid anchor"))
    .build()
    .unwrap_err();

assert_eq!(error.first().location, "/properties/inner/$anchor");
assert_eq!(error.first().rule, "meta/core#/$defs/anchorString/pattern");
assert_eq!(error.first().message, "must match /^[A-Za-z_][-A-Za-z0-9._]*$/");
```

Every violation is reported, not just the first, and `Display` lays them out:

```text
the built schema is not valid JSON Schema 2020-12 (2 violations)
  - /properties/a/$anchor: must match /^[A-Za-z_][-A-Za-z0-9._]*$/ [meta/core#/$defs/anchorString/pattern]
  - /properties/b/multipleOf: must be greater than 0 [meta/validation#/properties/multipleOf/exclusiveMinimum]
```

`build_unchecked()` skips the check; `Schema::validate()` runs it on demand; and
`validate_document(&Value)` checks a schema that came from somewhere else
entirely — a file on disk, an API response.

## `.build_frozen()` for schemas that are done

A built `Schema` is an ordinary owned value with public fields, so whoever holds
one can still edit it. That is right while a document is being assembled and
wrong once it is finished and shared. `build_frozen()` runs the same check and
wraps the result in a `FrozenSchema`: a cheaply cloned, `Send + Sync` handle
that hands out `&Schema` and nothing more.

```rust
let schema = Schema::object()
    .required_property("id", Schema::string().format(Format::Uuid))
    .build_frozen()?;

assert_eq!(schema.as_object().unwrap().required.as_ref().unwrap(), &["id"]);

let shared = schema.clone();       // shares the one schema, does not copy it
assert!(schema.is_same_as(&shared));
```

Reads work through `Deref` exactly as they do on a `Schema`. Writes do not
compile:

```rust
let mut frozen = Schema::string().build_frozen()?;
if let Schema::Object(object) = &mut *frozen {   // error[E0596]: cannot borrow
    object.title = Some("changed".into());       // data in dereference of
}                                                // `FrozenSchema` as mutable
```

Freezing protects the instance, not the information: `thaw()` clones a mutable
copy out and leaves the frozen original untouched. `Schema::freeze()` freezes
one you already hold — including an unchecked one, since freezing and validating
are separate concerns.

## Coverage

Every keyword of the 2020-12 dialect: the core vocabulary (`$schema`, `$id`,
`$ref`, `$dynamicRef`, `$anchor`, `$dynamicAnchor`, `$vocabulary`, `$comment`,
`$defs`), applicators (`allOf`, `anyOf`, `oneOf`, `not`, `if`/`then`/`else`,
`dependentSchemas`, `prefixItems`, `items`, `contains`, `properties`,
`patternProperties`, `additionalProperties`, `propertyNames`), the unevaluated
keywords, validation, metadata, `format`, and the content vocabulary.

## Output

`Schema` is `serde::Serialize`, so it drops into any serializer; `to_json()`,
`to_json_pretty()`, and `to_value()` are the shorthands. Output is
deterministic — keywords render in the specification's grouping order, and
`properties`, `$defs`, and the other named-subschema maps render in the order
the builder calls declared them, so a regenerated schema diffs cleanly.

## What this crate does not do

It **builds** schemas; it does not validate instances against them.

The meta-schema check constrains a schema's *shape* — that `$anchor` is a plain
name, that `$id` carries no fragment, that `multipleOf` is positive, that
`required` holds unique strings — and says nothing about whether the schema
makes sense. A `minLength` above its `maxLength`, a `then` with no `if`, or a
`$ref` to a definition that does not exist are all legal 2020-12 documents and
build without complaint. The crate never follows your references.

Documents nested past roughly fifty levels of subschema report a depth violation
rather than being checked to the bottom: the evaluator stops instead of risking
a stack overflow, which would abort the process uncatchably.

## Development

```sh
cargo test      # unit, integration, and doc tests
cargo clippy --all-targets -- -D warnings
cargo run --example order
```

## License

Apache-2.0.

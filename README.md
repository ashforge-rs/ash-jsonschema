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

## Schemas can be read, not only written

`Schema::parse()` reads a document back in, and `parse_checked()` reads and
validates in one step — keeping the two failure modes apart, since text that is
not JSON is a different problem from a document that parses and then is not a
schema:

```rust
let schema = Schema::parse(r#"{"type":"object","properties":{"id":{"type":"string"}}}"#)?;

assert!(schema.requires("id") == false);
assert!(schema.property("id").is_some());
assert_eq!(schema.pointer("/properties/id/type"), Some(json!("string")));
```

Parsing keeps everything the document arrived with. Keywords outside the
2020-12 dialect — OpenAPI's `discriminator`, an `x-` annotation, a private
validator's own attribute — land in `SchemaObject::extensions` rather than being
dropped, so a document parsed and rendered back out is unchanged. `.keyword()`
writes one from the builder side, and refuses a name the dialect already
defines rather than emitting a duplicate key.

A parse failure names the offending keyword and its position, rather than
reporting only that the document did not match:

```text
unknown variant `strng`, expected one of `null`, `boolean`, `object`, `array`,
`number`, `integer`, `string` at line 1 column 15
```

## A schema goes back into a builder

`into_builder()` picks a schema that already exists back up — parsed from a
file, thawed from a `FrozenSchema`, handed over by another crate — and resumes
the chain on it. `into_typed()` does the same under a type marker, which
reopens that type's vocabulary:

```rust
let base = Schema::parse(r#"{"type":"string","minLength":1}"#)?;
let tightened: StringBuilder = base.into_typed();
let schema = tightened.max_length(64).build()?;
```

`into_typed` asserts the shape rather than checking it: nothing confirms `type`
really says `string` before handing back a `StringBuilder`.

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

## `$ref` resolves against a `Registry`

A `$ref` is a URI reference, and following one needs a base to resolve against
and somewhere to look the answer up. A `Registry` is that somewhere: schemas go
in under the `$id` they declare, and references then resolve — across documents,
not only within one.

```rust
let address = Schema::object()
    .id("https://example.com/schemas/address")
    .required_property("city", Schema::string())
    .build()?;

let user = Schema::object()
    .id("https://example.com/schemas/user")
    .required_property("address", Schema::reference("address"))  // relative to $id
    .build()?;

let mut registry = Registry::new();
registry.insert(&address)?;
registry.insert(&user)?;

registry.check_references()?;   // every $ref lands somewhere
```

`resolve()` follows a reference to the subschema it names, by JSON Pointer or by
`$anchor`. Nested `$id`s become their own resources, resolved against the base
enclosing them, and `$dynamicAnchor` is indexed too — `resolve_dynamic()`
applies the outermost-wins rule that recursive schemas rely on.

A `$ref` naming a definition that does not exist is still a **legal** 2020-12
document, so `build()` keeps accepting it and `check_references()` is what
notices. The error says so in its own words rather than claiming the schema is
malformed:

```text
the schema has references that resolve to nothing (1 violation)
  - /properties/address/$ref: `#/$defs/Adress` resolves to nothing in the registry [registry/$ref]
```

Nothing is ever fetched. A registry resolves against what it was given, so a
reference to a document nobody inserted dangles rather than reaching the
network — retrieval is your policy, not the library's.

## A schema is a source, not just an output

Once a schema can be read and its references resolved, it becomes an input to
other tools.

### `diff` — what changed, and who it breaks

A schema is a contract between whoever writes the data and whoever reads it, and
a change lands on those two in opposite directions. Adding a required property
breaks writers and leaves readers alone; removing one is the mirror image. There
is no single "is this breaking?" answer, so a `Change` carries both and you ask
the question you actually have:

```rust
let report = diff(&v1, &v2);

assert!(report.is_breaking_for(Role::Writer));   // old data lacks the new field
assert!(!report.is_breaking_for(Role::Reader));  // old code ignores it
```

```text
3 changes
  - /properties/age: `minimum` tightened from 0 to 13 [writer: breaking, reader: compatible]
  - (root): removed property `nickname` [writer: compatible, reader: breaking]
  - (root): added required property `email` [writer: breaking, reader: compatible]
```

Classification is conservative. Where the effect cannot be read off the values —
two different `pattern`s, a reordered `oneOf` — the change is `Effect::Unknown`
rather than guessed at, and counts as breaking. A gate that waved through what
it could not classify would not be worth having.

`diff_resolved` follows `$ref`s, so a change inside a shared definition is
reported at each place that *uses* it. It takes a registry per side, because the
two schemas are different versions and a reference in each must resolve against
what that version knew.

### `bundle` — one file to ship

```rust
let bundled = bundle(&registry, "https://example.com/schemas/user")?;
```

Every referenced document is embedded under `$defs` keeping its own `$id`, which
is what makes the original `$ref`s still resolve — no reference is rewritten and
no `$id` is invented. The result is a legal 2020-12 document that resolves with
the originals absent, survives a round trip through text, and is unchanged by
bundling it again.

A reference that resolves nowhere is refused rather than bundled around, since
the alternative is a document that quietly does not work.

## Macros for the wordy shapes

The builder is the API; the macros are for where it makes you repeat
*structure*. There is no macro for `.min_length(1)` — a macro that wraps one
method call is just a second thing to learn.

`object!` writes `properties` and `required` from one list, which is the
duplication object schemas otherwise force:

```rust
let user = object! {
    required "id": Schema::string().format("uuid"),
    required "email": Schema::string().format("email"),
    "age": Schema::integer().minimum(0),
}
.title("User")
.closed()
.build()?;
```

The result is an ordinary `ObjectBuilder`, so the rest of the vocabulary still
chains off it and a nested `object!` needs no `.build()`. A property name can be
any expression when parenthesised — `required (tag): …` — for names that come
from a config rather than the source.

`defs!` takes a block of definitions, and `one_of_tagged!` writes the tagged
union JSON Schema has no keyword for: a `oneOf` where each branch pins a
discriminant with `const` and requires it.

```rust
let shape = one_of_tagged! {
    "kind";
    "circle" => object! { required "radius": Schema::number() },
    "rect"   => object! { required "width": Schema::number(),
                          required "height": Schema::number() },
}
.build()?;
```

Each macro produces byte-identical output to the chain it stands for, which the
test suite checks directly.

## `export` — an owned copy, from whatever you hold

```rust
let copy = schema.export();                          // a detached Schema
let inner = schema.export_at("/properties/address")  // a subtree, as a Schema
    .unwrap();
```

`export()` is on `Schema`, `FrozenSchema`, and every builder, so code that wants
a copy out need not know which it has — on a builder it takes one *without*
ending the chain, which is how a common base gets shared between several
schemas. `export_at` lifts a subschema out as a `Schema` rather than the bare
`Value` that `pointer()` returns; it yields `None` for a location that is not a
schema, so `/properties` (a map of them) does not count.

## Validating instances

The rest of the crate treats a schema as a document. `Validator` is the other
half: given a schema, does this JSON satisfy it?

```rust
let order = validator! {
    object! {
        required "id": Schema::string().min_length(1),
        required "total": Schema::integer().minimum(0),
    }
    .closed()
}?;

assert!(order.is_valid(&json!({"id": "a1", "total": 250})));
```

The schema is fixed once the validator holds it. There is no method that
replaces it, and `export_schema()` hands back a detached copy — to check against
something else, build another validator. Compiling is also where a *schema* can
be wrong: every `pattern` is compiled and every reference indexed up front, so a
validator that exists is one whose schema was understood, and `validate` has no
failure mode of its own.

### Two modes, one result type

`Mode::All` reports everything; `Mode::FailFast` stops at the first error.

```text
the instance is not valid (5 errors)
  - /items: must have at least 1 item [/properties/items/minItems]
  - /total/amount: must be at least 0 [/properties/total/properties/amount/minimum]
  - /total/currency: must match /^[A-Z]{3}$/ [/properties/total/properties/currency/pattern]
  - /total/vat: is not a permitted property [/properties/total/additionalProperties]
  - /extra: is not a permitted property [/additionalProperties]
```

Each error carries two pointers, because they answer different questions:
`location` is where in *your data* the problem is, `keyword` is which rule in the
*schema* rejected it. `outcome.at("/total")` filters to one part of the payload,
`check()` gives a `Result` for `?`, and `check_value()` takes anything
`Serialize`.

### Coverage

Every assertion and applicator of 2020-12, including `unevaluatedProperties` and
`unevaluatedItems` with the annotation collection they require, `$dynamicRef`
against the runtime dynamic scope, and `$ref` across documents via a `Registry`.

Measured against the official
[JSON-Schema-Test-Suite](https://github.com/json-schema-org/JSON-Schema-Test-Suite):
**1267 of 1278 draft2020-12 cases pass (99.1%)**. The remainder are custom
`$vocabulary` declarations that disable keywords, and `$dynamicRef` resolution
across several intermediate scopes.

Two things are deliberately not asserted: `format`, which 2020-12 defines as an
annotation by default, and the content vocabulary, which is likewise an
annotation. `pattern` uses `regex-lite`, so a pattern needing lookahead or
backreferences is reported at construction rather than silently not applying.

## What this crate does not do

It **builds**, **reads**, and **checks against** schemas. What it does not do is
fetch: no `$ref` is ever retrieved over the network, and no schema is generated
from a Rust type.

The meta-schema check constrains a schema's *shape* — that `$anchor` is a plain
name, that `$id` carries no fragment, that `multipleOf` is positive, that
`required` holds unique strings — and says nothing about whether the schema
makes sense. A `minLength` above its `maxLength`, or a `then` with no `if`, is a
legal 2020-12 document and builds without complaint.

References are followed only when you ask: `build()` does not resolve them, and
a `Registry` does. That split is deliberate, since a dangling `$ref` is legal
and a schema is often built before the documents it points at exist.

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

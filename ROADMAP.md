# Schema at the Center

The crate writes JSON Schema well. This is the plan for making the schema the
thing everything else is derived from — read, composed, resolved, turned into
other artifacts, and checked against.

**Status:** four of five phases shipped. 417 tests passing (all features), up
from 114 when this plan was written.

## The thesis

The crate's original identity was *a builder that happens to emit JSON Schema*.
The stronger identity is *a JSON Schema library whose front door happens to be a
builder*. That inverts what the core type is for: `Schema` stops being the
builder's output and becomes the value everything else consumes and produces.

The test for every proposal below: **does it make `Schema` more central, or does
it just add another way to write one?** Only the first kind earns its place.

Parsing was the precondition for all of it. A library cannot be built around a
type that can only be written and never read.

## The path

Five phases, genuinely ordered — each depends on what precedes it. The numbering
is load-bearing, not decorative.

| # | Phase | Depends on | Status |
| --- | --- | --- | --- |
| 01 | Reference resolution | parsing | shipped |
| 02 | Instance validation | 01 | shipped |
| 03 | Derive macro | nothing structural | **not planned** |
| 04 | Schema as source | 01 | shipped |
| 05 | Specification output | 02 | shipped |

### 01 — Reference resolution *(shipped)*

The README used to say plainly: "The crate never follows your references." It no
longer does. A `Registry` indexes schemas by the URIs they declare and resolves
against them — offline, with nothing fetched.

- `Registry` keyed by `$id`, with RFC 3986 base resolution promoted out of
  `validate.rs` into a shared `uri` module.
- `$anchor` and `$dynamicAnchor` indexing, generalised out of the meta-schema
  special case.
- `resolve`, `resolve_from`, `resolve_dynamic`, and `check_references` reporting
  each dangling ref at its own pointer.
- Cycles — self-referential and mutual — resolve without hanging.

**Two defects found by probing, not by reading.** `walk_subschemas` recursed
unbounded, and indexing cloned each resource's subtree — `serde_json::Value`
clones recursively, so a deep document aborted the process uncatchably.
Resources now share one `Arc` and point into it; the walk is depth-capped.

### 02 — Instance validation *(shipped)*

A separate evaluator rather than a generalised `validate.rs` — that module's
narrowness is what keeps `build()` honest, and destabilising it to serve a
second purpose would have been the wrong trade.

- Real annotation collection, so `unevaluatedProperties` and `unevaluatedItems`
  see through `$ref`, `allOf`, `anyOf` and `if`/`then` — including the rule that
  a failed branch evaluates nothing.
- `$dynamicRef` against the runtime dynamic scope, not a static index.
- `format` asserted on request, with custom handlers; TLD rules; calendar-aware
  date checking. `format` stays an annotation by default, as 2020-12 specifies.
- Not asserted: the content vocabulary, an annotation by default.

**The suite earned its place immediately.** The first run was 96.3%, and every
point after that was a real defect: counts written as `2.0` silently unenforced,
`const: null` read as an absent keyword, `contains` tracked as a high-water mark
rather than a set, percent-encoded pointers never decoded. None would have
surfaced from hand-written tests.

### 03 — Derive macro *(not planned)*

A companion `ash-jsonschema-derive` crate generating a schema from a Rust type,
reading `serde` attributes so one set of annotations drives both wire format and
schema.

**Dropped, on two grounds.** The hard part is not the macro — it is that Rust
types must map onto JSON Schema types, and that mapping is lossy in both
directions in ways no attribute syntax hides. And it competes directly with
`schemars`, which is entrenched and does that one job well.

The builder's advantage is schemas that *aren't* mirrors of Rust types: dynamic
ones, ones read from config, ones being diffed across versions. A derive macro
pulls toward the case where a competitor already wins.

### 04 — Schema as source *(shipped)*

A schema can be read, resolved, traversed, compared, and collapsed — which is
what "JSON Schema at the core" was meant to mean.

- Diffing, classified per audience rather than on one widening/narrowing axis.
  `diff_resolved` follows references and attributes a change to each use site.
- Bundling, by the method the specification describes — embed and keep the
  `$id`, rewrite nothing.
- Visitors: `walk::walk_subschemas`, built for the registry and reused unchanged
  by both.

**Two design errors caught by probing.** `diff_resolved` first took one
registry, which resolved both versions' references against the same documents
and silently reported no change. And it resolved refs with no base, so the
relative `#/$defs/Name` — the form nearly every schema writes — never resolved
at all.

### 05 — Specification output *(shipped)*

The error type was three loose strings, which made results unable to leave the
process and lost the structure of a branching failure.

- `Unit` is the specification's output unit: `instance_location`,
  `keyword_location`, `absolute_keyword_location`, `error`, `annotation`,
  nested `errors`.
- All four output formats — `Flag`, `Basic`, `Detailed`, `Verbose` — via
  `Outcome::output`.
- Errors are a tree internally, with `Outcome::errors` kept as the flat list of
  leaves so existing callers were unaffected.
- `Validator::annotate` collects annotations; `validate` does not, so the common
  question does not pay for an answer it will not read.

**Nesting is the point.** A failed `anyOf` can only say "no alternative
matched", which is the least useful thing a validator can report. Keeping each
branch's reason turns that into `` `iban` is required``.

**Found by conformance testing, not by reading.** The suite's own output tests
are self-validating — each expectation is a JSON Schema the output must satisfy
— so the validator checks itself: 4/4. Against the main suite it also surfaced a
pre-existing parser bug, `{"minItems": 1.0}` rejected as `expected u64`, when
JSON has one number type and `1.0` is a valid non-negative integer.

## Deliberately not doing

**Other dialects.** One dialect is the crate's stated design premise and its
main simplification. Draft-07 support would reintroduce exactly the selector
threading the README rejects.

**Network fetching of `$ref`.** Offline and reproducible is a feature. The
registry should take schemas the caller supplies; fetching is the caller's
policy, not the library's.

**Semantic lint rules.** Tempting — `minLength` above `maxLength`, a `then` with
no `if`. But the meta-schema boundary is currently crisp and easy to explain. If
added, it belongs behind an explicit `lint()` call, never inside `build()`.

**Payload generation.** Producing example instances from a schema is a
different problem — satisfying constraints is search, not traversal — and the
useful version needs constraint solving the crate has no reason to carry.

## What could come next

None of these are commitments.

**Structured error data.** `ValidationError` now carries the spec's locations,
but `message` is still English prose. A machine that wants to know *which bound
was violated and by how much* has to parse it.

**Enforceable compatibility.** `diff` computes `Effect::{Compatible, Breaking,
Unknown}` but cannot be enforced. A `Report::assert_compatible(Role)` plus a
baseline-file convention turns an analysis library into a CI policy tool. This
is the crate's most distinctive asset and the cheapest thing left to build on.

**Instances without `serde_json::Value`.** The validator takes `&Value`, so
validating means allocating the whole document first. A trait over the instance
would let it read from `simd-json` or a borrowed slice. Only pays off under
load.

**The two documented format limits.** `xn--` punycode labels are checked for
shape but not decoded, and quoted email local parts are rejected rather than
half-parsed. Both need full IDNA rules and a dependency; currently documented
rather than implemented.

## Verification

Status reflects the working tree at time of writing.

```
cargo test                        408 tests
cargo test --no-default-features  278 tests
cargo test --all-features         417 tests
cargo clippy --all-targets --all-features -- -D warnings   clean
cargo doc --no-deps                                        clean, all configs
```

Against the official
[JSON-Schema-Test-Suite](https://github.com/json-schema-org/JSON-Schema-Test-Suite):

| Suite | Result |
| --- | --- |
| draft2020-12 | 1281 / 1299 (98.6%) |
| output tests | 4 / 4 |

The 18 remaining failures are custom `$vocabulary` declarations that disable
keywords, `$dynamicRef` resolution across several intermediate scopes, and five
patterns needing Unicode property escapes (`\p{Letter}`), which `regex-lite`
does not implement.

## A correction worth keeping

Twice, while this plan was being executed, phase 02 was recommended for cutting
on the grounds that a correct 2020-12 evaluator is a large project against a
mature incumbent.

**That recommendation was wrong.** The registry, walk, and URI modules built for
other phases carried most of the weight, and the official suite made correctness
measurable rather than a matter of judgement. The caution about scope was fair;
the conclusion drawn from it was not.

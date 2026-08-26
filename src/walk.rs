//! Where subschemas actually live in a schema document.
//!
//! A schema is a JSON object, and so are several of the things *inside* a
//! schema that are not schemas at all. `properties` is a map of names to
//! subschemas, so `/properties/items` is a schema; but a property may legally
//! be named `items`, `$ref`, or `$id`, and none of those are keywords in that
//! position. A walk that recursed into every object it met would treat
//! `{"properties": {"$id": {"type": "string"}}}` as a nested resource
//! declaration, which it is not.
//!
//! This module encodes the 2020-12 applicator structure so a traversal descends
//! only where a subschema can be. The three shapes a keyword's value can take:
//!
//! | Shape | Keywords |
//! |---|---|
//! | one subschema | `items`, `not`, `if`, `contains`, … |
//! | a list of subschemas | `allOf`, `anyOf`, `oneOf`, `prefixItems` |
//! | a map of names to subschemas | `properties`, `$defs`, `patternProperties`, … |

use serde_json::Value;

use crate::meta::escape_pointer_token;

/// How deep a walk will descend before stopping.
///
/// A schema nested past this is not traversed to the bottom. The cap exists
/// because unbounded recursion on a hostile or generated document overflows the
/// stack, which aborts the process and cannot be caught — a truncated walk is a
/// far better failure.
///
/// It is set well above anything a hand-written schema reaches, and above the
/// 128-level cap `serde_json` itself enforces while parsing, so a document that
/// came from [`Schema::parse`](crate::Schema::parse) can never hit it. Only a
/// [`Value`] built up in memory can.
pub const MAX_DEPTH: usize = 256;

/// Keywords whose value is itself a schema.
pub const SUBSCHEMA: [&str; 11] = [
    "additionalProperties",
    "contains",
    "contentSchema",
    "else",
    "if",
    "items",
    "not",
    "propertyNames",
    "then",
    "unevaluatedItems",
    "unevaluatedProperties",
];

/// Keywords whose value is an array of schemas.
pub const SUBSCHEMA_LIST: [&str; 4] = ["allOf", "anyOf", "oneOf", "prefixItems"];

/// Keywords whose value is an object mapping names to schemas.
pub const SUBSCHEMA_MAP: [&str; 4] = [
    "$defs",
    "dependentSchemas",
    "patternProperties",
    "properties",
];

/// Calls `visit` on `schema` and on every subschema beneath it, depth-first,
/// with the JSON Pointer that locates each one.
///
/// The root is visited with the pointer `""`. Only real subschema positions are
/// descended into, so a property whose *name* collides with a keyword is not
/// mistaken for one:
///
/// ```
/// # use ash_jsonschema_builder::walk::walk_subschemas;
/// let document = serde_json::json!({
///     "properties": {
///         "not": {"type": "string"}
///     }
/// });
///
/// let mut seen = Vec::new();
/// walk_subschemas(&document, |pointer, _| seen.push(pointer.to_owned()));
///
/// // `/properties/not` is a schema; the `not` *keyword* was never there.
/// assert_eq!(seen, ["", "/properties/not"]);
/// ```
///
/// Descent stops at [`MAX_DEPTH`], so a pathologically nested document
/// truncates rather than overflowing the stack.
///
/// A boolean is a schema too, and is visited like any other:
///
/// ```
/// # use ash_jsonschema_builder::walk::walk_subschemas;
/// let document = serde_json::json!({"items": true});
///
/// let mut seen = Vec::new();
/// walk_subschemas(&document, |pointer, _| seen.push(pointer.to_owned()));
/// assert_eq!(seen, ["", "/items"]);
/// ```
pub fn walk_subschemas<'a, F>(schema: &'a Value, mut visit: F)
where
    F: FnMut(&str, &'a Value),
{
    walk_from(schema, String::new(), 0, &mut visit);
}

fn walk_from<'a, F>(schema: &'a Value, pointer: String, depth: usize, visit: &mut F)
where
    F: FnMut(&str, &'a Value),
{
    visit(&pointer, schema);

    // Stop rather than risk a stack overflow, which aborts uncatchably.
    if depth >= MAX_DEPTH {
        return;
    }

    // Only the object form has keywords to descend through; `true` and `false`
    // are leaves.
    let Some(object) = schema.as_object() else {
        return;
    };
    let depth = depth + 1;

    for keyword in SUBSCHEMA {
        if let Some(child) = object.get(keyword) {
            walk_from(child, format!("{pointer}/{keyword}"), depth, visit);
        }
    }

    for keyword in SUBSCHEMA_LIST {
        let Some(Value::Array(items)) = object.get(keyword) else {
            continue;
        };
        for (index, item) in items.iter().enumerate() {
            walk_from(item, format!("{pointer}/{keyword}/{index}"), depth, visit);
        }
    }

    for keyword in SUBSCHEMA_MAP {
        let Some(Value::Object(entries)) = object.get(keyword) else {
            continue;
        };
        for (name, child) in entries {
            let token = escape_pointer_token(name);
            walk_from(child, format!("{pointer}/{keyword}/{token}"), depth, visit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pointers(document: &Value) -> Vec<String> {
        let mut seen = Vec::new();
        walk_subschemas(document, |pointer, _| seen.push(pointer.to_owned()));
        seen
    }

    #[test]
    fn the_root_is_visited_even_with_no_subschemas() {
        assert_eq!(pointers(&serde_json::json!({"type": "string"})), [""]);
        assert_eq!(pointers(&serde_json::json!(true)), [""]);
    }

    #[test]
    fn each_keyword_shape_is_descended() {
        let document = serde_json::json!({
            "not": {"type": "null"},
            "allOf": [{"type": "string"}, {"minLength": 1}],
            "properties": {"a": {"type": "integer"}},
        });

        assert_eq!(
            pointers(&document),
            [
                "",
                "/not",
                "/allOf/0",
                "/allOf/1",
                "/properties/a",
            ],
        );
    }

    #[test]
    fn a_property_named_like_a_keyword_is_not_treated_as_one() {
        // Every one of these is a property *name*, not a keyword position.
        let document = serde_json::json!({
            "properties": {
                "not": {"type": "string"},
                "$id": {"type": "string"},
                "allOf": {"type": "string"},
            }
        });

        // `serde_json::Value` holds objects in a `BTreeMap` unless the
        // `preserve_order` feature is on, so the order subschema *maps* come
        // out in is the key order, not the author's. Compare as a set.
        let mut seen = pointers(&document);
        seen.sort();
        assert_eq!(
            seen,
            [
                "",
                "/properties/$id",
                "/properties/allOf",
                "/properties/not",
            ],
        );
    }

    #[test]
    fn property_names_needing_escapes_produce_valid_pointers() {
        let document = serde_json::json!({
            "properties": {"a/b": {"type": "string"}, "c~d": {"type": "string"}}
        });

        let mut seen = pointers(&document);
        seen.sort();
        assert_eq!(seen, ["", "/properties/a~1b", "/properties/c~0d"]);
    }

    #[test]
    fn the_pointers_a_walk_yields_all_resolve() {
        let document = serde_json::json!({
            "$defs": {"Money": {"type": "object"}},
            "properties": {"a/b": {"items": {"type": "string"}}},
            "prefixItems": [{"type": "number"}],
        });

        for pointer in pointers(&document) {
            assert!(
                crate::meta::resolve_pointer(&document, &pointer).is_some(),
                "the walk produced `{pointer}`, which does not resolve",
            );
        }
    }

    #[test]
    fn a_keyword_holding_the_wrong_shape_is_skipped_rather_than_panicking() {
        // Not legal 2020-12, but the walk must not care — it is used on
        // documents that have not been validated yet.
        let document = serde_json::json!({
            "allOf": {"type": "string"},
            "properties": ["not a map"],
        });

        assert_eq!(pointers(&document), [""]);
    }

    #[test]
    fn descent_stops_at_the_depth_cap() {
        // Past `MAX_DEPTH`, but shallow enough that building and dropping the
        // `Value` itself — both recursive — stay well inside the stack.
        let depth = MAX_DEPTH + 32;
        let mut document = serde_json::json!({"type": "string"});
        for _ in 0..depth {
            document = serde_json::json!({"properties": {"n": document}});
        }

        // The root, plus one visit per level descended, and no more: the walk
        // truncates instead of running to the bottom.
        let seen = pointers(&document);
        assert_eq!(seen.len(), MAX_DEPTH + 1);

        // The deepest pointer reached is exactly at the cap.
        let deepest = seen.last().unwrap();
        assert_eq!(deepest.matches("/properties/").count(), MAX_DEPTH);
    }

    #[test]
    fn nesting_composes_through_every_shape() {
        let document = serde_json::json!({
            "properties": {
                "outer": {
                    "allOf": [{"items": {"type": "string"}}]
                }
            }
        });

        assert_eq!(
            pointers(&document),
            [
                "",
                "/properties/outer",
                "/properties/outer/allOf/0",
                "/properties/outer/allOf/0/items",
            ],
        );
    }
}

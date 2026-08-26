//! Checks a built schema against the embedded 2020-12 meta-schema.
//!
//! This is a JSON Schema evaluator, but a deliberately narrow one: the *schema*
//! side is always the meta-schema from [`crate::meta`], and only the *instance*
//! side — the document you just built — varies. That bounds the keyword set it
//! has to understand to the ones the eight vendored documents actually use, and
//! [`tests::the_meta_schema_uses_no_keyword_this_module_ignores`] fails the
//! build if a future respin of those documents reaches for anything else.
//!
//! In particular `unevaluatedProperties` and `unevaluatedItems` are not
//! implemented, because the meta-schema does not use them. Do not lift this
//! module out to validate instances against arbitrary schemas.
//!
//! The walk is depth-first over your document, so a violation's
//! [`location`](Violation::location) is the JSON Pointer of the subschema that
//! broke a rule, however deeply nested — the root document and every subschema
//! under it are each checked in turn.

use serde_json::{Map, Value};

use crate::error::{SchemaError, Violation};
use crate::meta::{MetaSchema, escape_pointer_token, resolve_pointer};

/// How deep the evaluator will recurse before giving up.
///
/// Each level of schema nesting costs five of these — the keyword descent into
/// the subschema, the `$ref` hops through the vocabulary documents, and the
/// `$dynamicRef` back to the dialect root — so this permits roughly fifty
/// levels of nested subschemas.
///
/// The cap exists because a stack overflow aborts the process and cannot be
/// caught, whereas hitting this yields an ordinary [`Violation`]. It is set
/// with a wide margin below what a debug build survives on the 2 MiB stack a
/// test thread gets, which is the tightest case; release builds have far
/// smaller frames.
const MAX_DEPTH: usize = 256;

/// The prefix every meta-schema URI shares, trimmed from reported rules.
const URI_PREFIX: &str = "https://json-schema.org/draft/2020-12/";

/// Checks an already-parsed JSON document against the embedded JSON Schema
/// 2020-12 meta-schema.
///
/// [`Schema::validate`](crate::Schema::validate) is this, applied to a schema
/// this crate built. Reach for this one when the document came from somewhere
/// else — a file on disk, an API response — and you want to know whether it is
/// a well-formed schema before working with it.
///
/// ```
/// # use ash_jsonschema::validate_document;
/// let good = serde_json::json!({"type": "string", "minLength": 1});
/// assert!(validate_document(&good).is_ok());
///
/// let bad = serde_json::json!({"type": "string", "$anchor": "not an anchor"});
/// assert_eq!(validate_document(&bad).unwrap_err().first().location, "/$anchor");
/// ```
pub fn validate_document(document: &Value) -> Result<(), SchemaError> {
    let meta = MetaSchema::current();
    let mut validator = Validator {
        meta,
        violations: Vec::new(),
        silent: 0,
        depth: 0,
    };

    let root = meta.root();
    let scope = Scope {
        base: crate::schema::DIALECT.to_owned(),
        dynamic: vec![crate::schema::DIALECT.to_owned()],
    };
    validator.eval(root, &scope, document, "", "schema#");

    match validator.violations.is_empty() {
        true => Ok(()),
        false => Err(SchemaError::new(validator.violations)),
    }
}

/// The schema resources entered on the way to the schema being evaluated.
#[derive(Clone, Debug)]
struct Scope {
    /// The current resource's URI — what a `$ref` resolves against.
    base: String,
    /// Every resource entered, outermost first. `$dynamicRef` searches this.
    dynamic: Vec<String>,
}

struct Validator<'m> {
    meta: &'m MetaSchema,
    violations: Vec<Violation>,
    /// Non-zero while evaluating a branch whose failure is not itself an error
    /// — an `anyOf` alternative, an `if` condition, the subject of a `not`.
    silent: usize,
    depth: usize,
}

impl<'m> Validator<'m> {
    fn report(&mut self, location: &str, rule: &str, message: impl Into<String>) {
        if self.silent == 0 {
            self.violations.push(Violation {
                location: location.to_owned(),
                rule: rule.to_owned(),
                message: message.into(),
            });
        }
    }

    /// Evaluates `schema` against `instance`, returning whether it matched.
    ///
    /// `location` is a JSON Pointer into the document being checked; `rule` is
    /// the meta-schema location, for reporting.
    fn eval(
        &mut self,
        schema: &'m Value,
        scope: &Scope,
        instance: &Value,
        location: &str,
        rule: &str,
    ) -> bool {
        let map = match schema {
            Value::Bool(true) => return true,
            Value::Bool(false) => {
                self.report(location, rule, "nothing is valid here");
                return false;
            }
            Value::Object(map) => map,
            other => panic!("the meta-schema holds a non-schema at {rule}: {other}"),
        };

        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            let message = format!("nested too deeply to check (past {MAX_DEPTH} evaluation steps)");
            self.report(location, rule, message);
            return false;
        }

        // A nested `$id` starts a new resource: later `$ref`s resolve against
        // it, and it joins the dynamic scope.
        let entered;
        let scope = match map.get("$id").and_then(Value::as_str) {
            Some(id) if id != scope.base => {
                let (base, _) = crate::uri::resolve(&scope.base, id);
                let mut dynamic = scope.dynamic.clone();
                dynamic.push(base.clone());
                entered = Scope { base, dynamic };
                &entered
            }
            _ => scope,
        };

        let valid = self.eval_keywords(map, scope, instance, location, rule);
        self.depth -= 1;
        valid
    }

    fn eval_keywords(
        &mut self,
        map: &'m Map<String, Value>,
        scope: &Scope,
        instance: &Value,
        location: &str,
        rule: &str,
    ) -> bool {
        let mut valid = true;

        // --- references ---------------------------------------------------
        if let Some(target) = map.get("$ref").and_then(Value::as_str) {
            let (resource, pointer) = self.resolve_reference(&scope.base, target, rule);
            valid &= self.eval_resource(&resource, &pointer, scope, instance, location);
        }
        if let Some(target) = map.get("$dynamicRef").and_then(Value::as_str) {
            let (resource, pointer) = self.resolve_dynamic(scope, target, rule);
            valid &= self.eval_resource(&resource, &pointer, scope, instance, location);
        }

        // --- assertions on any instance -----------------------------------
        if let Some(types) = map.get("type") {
            valid &= self.check_type(types, instance, location, rule);
        }
        if let Some(Value::Array(values)) = map.get("enum")
            && !values.iter().any(|value| json_eq(value, instance))
        {
            self.report(
                location,
                &child(rule, "enum"),
                format!("not one of {}", brief(values)),
            );
            valid = false;
        }
        if let Some(expected) = map.get("const")
            && !json_eq(expected, instance)
        {
            self.report(
                location,
                &child(rule, "const"),
                format!("must be {}", one_line(expected)),
            );
            valid = false;
        }

        valid &= self.check_numeric(map, instance, location, rule);
        valid &= self.check_string(map, instance, location, rule);
        valid &= self.check_array(map, scope, instance, location, rule);
        valid &= self.check_object(map, scope, instance, location, rule);
        valid &= self.check_applicators(map, scope, instance, location, rule);

        valid
    }

    // --- reference resolution --------------------------------------------

    /// Splits a `$ref` into the resource it names and the pointer within it.
    fn resolve_reference(&self, base: &str, target: &str, rule: &str) -> (String, String) {
        let (resource, fragment) = crate::uri::resolve(base, target);
        let pointer = self.fragment_to_pointer(&resource, &fragment, rule, false);
        (resource, pointer)
    }

    /// Resolves a `$dynamicRef`. When the initial target is a `$dynamicAnchor`,
    /// the *outermost* resource in the dynamic scope carrying that anchor wins
    /// — which is what lets the meta-schema hand subschema validation back to
    /// whichever dialect started the evaluation.
    fn resolve_dynamic(&self, scope: &Scope, target: &str, rule: &str) -> (String, String) {
        let (resource, fragment) = crate::uri::resolve(&scope.base, target);
        let is_plain_name = !fragment.is_empty() && !fragment.starts_with('/');

        if is_plain_name && self.meta.dynamic_anchor(&resource, &fragment).is_some() {
            for outer in &scope.dynamic {
                if let Some(pointer) = self.meta.dynamic_anchor(outer, &fragment) {
                    return (outer.clone(), pointer.to_owned());
                }
            }
        }

        let pointer = self.fragment_to_pointer(&resource, &fragment, rule, true);
        (resource, pointer)
    }

    fn fragment_to_pointer(
        &self,
        resource: &str,
        fragment: &str,
        rule: &str,
        dynamic: bool,
    ) -> String {
        if fragment.is_empty() || fragment.starts_with('/') {
            return fragment.to_owned();
        }
        let found = match dynamic {
            true => self.meta.dynamic_anchor(resource, fragment),
            false => self.meta.anchor(resource, fragment),
        };
        found
            .unwrap_or_else(|| {
                panic!("the meta-schema at {rule} names a missing anchor #{fragment}")
            })
            .to_owned()
    }

    /// Evaluates the schema a reference landed on, entering its resource.
    fn eval_resource(
        &mut self,
        resource: &str,
        pointer: &str,
        scope: &Scope,
        instance: &Value,
        location: &str,
    ) -> bool {
        let document = self.meta.document(resource).unwrap_or_else(|| {
            panic!("the meta-schema references an unembedded document {resource}")
        });
        let target = resolve_pointer(document, pointer)
            .unwrap_or_else(|| panic!("the meta-schema references a missing {resource}#{pointer}"));

        // The target names its own resource, so the rule location restarts
        // there rather than accumulating one segment per `$ref` hop.
        let rule = format!(
            "{}#{pointer}",
            resource.strip_prefix(URI_PREFIX).unwrap_or(resource)
        );

        let mut dynamic = scope.dynamic.clone();
        if !dynamic.iter().any(|entered| entered == resource) {
            dynamic.push(resource.to_owned());
        }
        let scope = Scope {
            base: resource.to_owned(),
            dynamic,
        };

        self.eval(target, &scope, instance, location, &rule)
    }

    // --- assertions -------------------------------------------------------

    fn check_type(&mut self, types: &Value, instance: &Value, location: &str, rule: &str) -> bool {
        let matches = match types {
            Value::String(one) => type_matches(one, instance),
            Value::Array(many) => many
                .iter()
                .filter_map(Value::as_str)
                .any(|one| type_matches(one, instance)),
            _ => true,
        };
        if !matches {
            let expected = match types {
                Value::String(one) => one.clone(),
                other => one_line(other),
            };
            self.report(
                location,
                &child(rule, "type"),
                format!("must be of type {expected}, found {}", type_name(instance)),
            );
        }
        matches
    }

    fn check_numeric(
        &mut self,
        map: &Map<String, Value>,
        instance: &Value,
        location: &str,
        rule: &str,
    ) -> bool {
        let Some(value) = instance.as_f64() else {
            return true;
        };
        let mut valid = true;

        /// A numeric bound: its keyword, the test it imposes, and how to say so.
        type Bound = (&'static str, fn(f64, f64) -> bool, &'static str);

        let bounds: [Bound; 4] = [
            ("minimum", |v, b| v >= b, "must be at least"),
            ("maximum", |v, b| v <= b, "must be at most"),
            ("exclusiveMinimum", |v, b| v > b, "must be greater than"),
            ("exclusiveMaximum", |v, b| v < b, "must be less than"),
        ];
        for (keyword, holds, phrase) in bounds {
            if let Some(bound) = map.get(keyword).and_then(Value::as_f64)
                && !holds(value, bound)
            {
                self.report(location, &child(rule, keyword), format!("{phrase} {bound}"));
                valid = false;
            }
        }

        if let Some(divisor) = map.get("multipleOf").and_then(Value::as_f64)
            && (divisor <= 0.0 || (value / divisor).fract() != 0.0)
        {
            self.report(
                location,
                &child(rule, "multipleOf"),
                format!("must be a multiple of {divisor}"),
            );
            valid = false;
        }

        valid
    }

    fn check_string(
        &mut self,
        map: &Map<String, Value>,
        instance: &Value,
        location: &str,
        rule: &str,
    ) -> bool {
        let Some(text) = instance.as_str() else {
            return true;
        };
        let mut valid = true;

        // Lengths are counted in Unicode code points, not bytes.
        let length = || text.chars().count() as u64;
        if let Some(min) = map.get("minLength").and_then(Value::as_u64)
            && length() < min
        {
            self.report(
                location,
                &child(rule, "minLength"),
                format!("must be at least {min} characters"),
            );
            valid = false;
        }
        if let Some(max) = map.get("maxLength").and_then(Value::as_u64)
            && length() > max
        {
            self.report(
                location,
                &child(rule, "maxLength"),
                format!("must be at most {max} characters"),
            );
            valid = false;
        }
        if let Some(pattern) = map.get("pattern").and_then(Value::as_str)
            && !self.meta.regex(pattern).is_match(text)
        {
            self.report(
                location,
                &child(rule, "pattern"),
                format!("must match /{pattern}/"),
            );
            valid = false;
        }

        valid
    }

    fn check_array(
        &mut self,
        map: &'m Map<String, Value>,
        scope: &Scope,
        instance: &Value,
        location: &str,
        rule: &str,
    ) -> bool {
        let Some(items) = instance.as_array() else {
            return true;
        };
        let mut valid = true;

        if let Some(min) = map.get("minItems").and_then(Value::as_u64)
            && (items.len() as u64) < min
        {
            self.report(
                location,
                &child(rule, "minItems"),
                format!("must have at least {min} items"),
            );
            valid = false;
        }
        if let Some(max) = map.get("maxItems").and_then(Value::as_u64)
            && (items.len() as u64) > max
        {
            self.report(
                location,
                &child(rule, "maxItems"),
                format!("must have at most {max} items"),
            );
            valid = false;
        }
        if map.get("uniqueItems") == Some(&Value::Bool(true))
            && let Some(duplicate) = first_duplicate(items)
        {
            self.report(
                location,
                &child(rule, "uniqueItems"),
                format!("items must be unique, {} is repeated", one_line(duplicate)),
            );
            valid = false;
        }

        let mut covered = 0;
        if let Some(Value::Array(prefix)) = map.get("prefixItems") {
            covered = prefix.len().min(items.len());
            for (index, schema) in prefix.iter().take(covered).enumerate() {
                let rule = child(&child(rule, "prefixItems"), &index.to_string());
                valid &= self.eval(
                    schema,
                    scope,
                    &items[index],
                    &index_location(location, index),
                    &rule,
                );
            }
        }
        if let Some(schema) = map.get("items") {
            let rule = child(rule, "items");
            for (index, item) in items.iter().enumerate().skip(covered) {
                valid &= self.eval(schema, scope, item, &index_location(location, index), &rule);
            }
        }
        if let Some(schema) = map.get("contains") {
            let rule = child(rule, "contains");
            let matched = items
                .iter()
                .enumerate()
                .filter(|(index, item)| {
                    self.silent += 1;
                    let ok = self.eval(
                        schema,
                        scope,
                        item,
                        &index_location(location, *index),
                        &rule,
                    );
                    self.silent -= 1;
                    ok
                })
                .count() as u64;

            let min = map.get("minContains").and_then(Value::as_u64).unwrap_or(1);
            let max = map
                .get("maxContains")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX);
            if matched < min || matched > max {
                self.report(
                    location,
                    &rule,
                    format!("{matched} items match `contains`, expected {min}..={max}"),
                );
                valid = false;
            }
        }

        valid
    }

    fn check_object(
        &mut self,
        map: &'m Map<String, Value>,
        scope: &Scope,
        instance: &Value,
        location: &str,
        rule: &str,
    ) -> bool {
        let Some(object) = instance.as_object() else {
            return true;
        };
        let mut valid = true;

        if let Some(min) = map.get("minProperties").and_then(Value::as_u64)
            && (object.len() as u64) < min
        {
            self.report(
                location,
                &child(rule, "minProperties"),
                format!("must have at least {min} properties"),
            );
            valid = false;
        }
        if let Some(max) = map.get("maxProperties").and_then(Value::as_u64)
            && (object.len() as u64) > max
        {
            self.report(
                location,
                &child(rule, "maxProperties"),
                format!("must have at most {max} properties"),
            );
            valid = false;
        }
        if let Some(Value::Array(names)) = map.get("required") {
            for name in names.iter().filter_map(Value::as_str) {
                if !object.contains_key(name) {
                    self.report(
                        location,
                        &child(rule, "required"),
                        format!("`{name}` is required"),
                    );
                    valid = false;
                }
            }
        }
        if let Some(Value::Object(dependencies)) = map.get("dependentRequired") {
            for (trigger, names) in dependencies.iter().filter(|(t, _)| object.contains_key(*t)) {
                for name in names
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    if !object.contains_key(name) {
                        let rule = child(&child(rule, "dependentRequired"), trigger);
                        self.report(
                            location,
                            &rule,
                            format!("`{name}` is required alongside `{trigger}`"),
                        );
                        valid = false;
                    }
                }
            }
        }

        // Track which properties `properties` and `patternProperties` claimed,
        // so `additionalProperties` sees only what is left.
        let mut claimed: Vec<&str> = Vec::new();

        if let Some(Value::Object(properties)) = map.get("properties") {
            for (name, schema) in properties {
                if let Some(child_instance) = object.get(name) {
                    claimed.push(name);
                    let rule = child(&child(rule, "properties"), name);
                    valid &= self.eval(
                        schema,
                        scope,
                        child_instance,
                        &member(location, name),
                        &rule,
                    );
                }
            }
        }
        if let Some(Value::Object(patterns)) = map.get("patternProperties") {
            for (pattern, schema) in patterns {
                let regex = self.meta.regex(pattern);
                let matching: Vec<&String> =
                    object.keys().filter(|name| regex.is_match(name)).collect();
                for name in matching {
                    claimed.push(name);
                    let rule = child(&child(rule, "patternProperties"), pattern);
                    let child_instance = &object[name];
                    valid &= self.eval(
                        schema,
                        scope,
                        child_instance,
                        &member(location, name),
                        &rule,
                    );
                }
            }
        }
        if let Some(schema) = map.get("additionalProperties") {
            let rule = child(rule, "additionalProperties");
            for (name, child_instance) in object
                .iter()
                .filter(|(n, _)| !claimed.contains(&n.as_str()))
            {
                valid &= self.eval(
                    schema,
                    scope,
                    child_instance,
                    &member(location, name),
                    &rule,
                );
            }
        }
        if let Some(schema) = map.get("propertyNames") {
            let rule = child(rule, "propertyNames");
            for name in object.keys() {
                let name_instance = Value::String(name.clone());
                valid &= self.eval(
                    schema,
                    scope,
                    &name_instance,
                    &member(location, name),
                    &rule,
                );
            }
        }
        if let Some(Value::Object(dependencies)) = map.get("dependentSchemas") {
            for (trigger, schema) in dependencies.iter().filter(|(t, _)| object.contains_key(*t)) {
                let rule = child(&child(rule, "dependentSchemas"), trigger);
                valid &= self.eval(schema, scope, instance, location, &rule);
            }
        }

        valid
    }

    fn check_applicators(
        &mut self,
        map: &'m Map<String, Value>,
        scope: &Scope,
        instance: &Value,
        location: &str,
        rule: &str,
    ) -> bool {
        let mut valid = true;

        // Every `allOf` branch must hold, so its failures are reported as they
        // are found rather than summarised.
        if let Some(Value::Array(branches)) = map.get("allOf") {
            for (index, branch) in branches.iter().enumerate() {
                let rule = child(&child(rule, "allOf"), &index.to_string());
                valid &= self.eval(branch, scope, instance, location, &rule);
            }
        }

        // For the choice applicators a failing branch is not itself an error —
        // only the choice as a whole is — so branches evaluate silently and one
        // violation stands for the lot.
        if let Some(Value::Array(branches)) = map.get("anyOf") {
            let rule = child(rule, "anyOf");
            let any = branches.iter().enumerate().any(|(index, branch)| {
                self.silent += 1;
                let rule = child(&rule, &index.to_string());
                let ok = self.eval(branch, scope, instance, location, &rule);
                self.silent -= 1;
                ok
            });
            if !any {
                self.report(location, &rule, "matches none of the permitted forms");
                valid = false;
            }
        }
        if let Some(Value::Array(branches)) = map.get("oneOf") {
            let rule = child(rule, "oneOf");
            let matched = branches
                .iter()
                .enumerate()
                .filter(|(index, branch)| {
                    self.silent += 1;
                    let rule = child(&rule, &index.to_string());
                    let ok = self.eval(branch, scope, instance, location, &rule);
                    self.silent -= 1;
                    ok
                })
                .count();
            if matched != 1 {
                self.report(
                    location,
                    &rule,
                    format!("matches {matched} of the permitted forms, expected exactly 1"),
                );
                valid = false;
            }
        }
        if let Some(schema) = map.get("not") {
            let rule = child(rule, "not");
            self.silent += 1;
            let matched = self.eval(schema, scope, instance, location, &rule);
            self.silent -= 1;
            if matched {
                self.report(location, &rule, "matches a forbidden form");
                valid = false;
            }
        }
        if let Some(condition) = map.get("if") {
            self.silent += 1;
            let holds = self.eval(condition, scope, instance, location, &child(rule, "if"));
            self.silent -= 1;

            let branch = if holds { "then" } else { "else" };
            if let Some(schema) = map.get(branch) {
                valid &= self.eval(schema, scope, instance, location, &child(rule, branch));
            }
        }

        valid
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Appends a reference token to a JSON Pointer.
fn child(pointer: &str, token: &str) -> String {
    format!("{pointer}/{}", escape_pointer_token(token))
}

/// The pointer to a named member of the instance at `location`.
fn member(location: &str, name: &str) -> String {
    child(location, name)
}

/// The pointer to an indexed element of the instance at `location`.
fn index_location(location: &str, index: usize) -> String {
    format!("{location}/{index}")
}

/// The `type` name JSON Schema uses for an instance.
fn type_name(instance: &Value) -> &'static str {
    match instance {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Whether `instance` is of the named JSON Schema type.
fn type_matches(name: &str, instance: &Value) -> bool {
    match name {
        // `integer` is a number with a zero fractional part, not a distinct
        // JSON type — `1.0` satisfies it.
        "integer" => instance.as_f64().is_some_and(|value| value.fract() == 0.0),
        "number" => instance.is_number(),
        other => other == type_name(instance),
    }
}

/// JSON Schema equality, which compares numbers by value rather than by the
/// representation they were parsed from.
fn json_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => a == b,
            _ => a == b,
        },
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| json_eq(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, a)| b.get(key).is_some_and(|b| json_eq(a, b)))
        }
        _ => left == right,
    }
}

/// The first element that repeats an earlier one, under JSON equality.
fn first_duplicate(items: &[Value]) -> Option<&Value> {
    items
        .iter()
        .enumerate()
        .find(|(index, item)| items[..*index].iter().any(|earlier| json_eq(earlier, item)))
        .map(|(_, item)| item)
}

/// A value on one line, for a message.
fn one_line(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "?".to_owned())
}

/// A list of values on one line, elided if long.
fn brief(values: &[Value]) -> String {
    const SHOWN: usize = 6;
    let head: Vec<String> = values.iter().take(SHOWN).map(one_line).collect();
    match values.len() > SHOWN {
        true => format!("[{}, …]", head.join(", ")),
        false => format!("[{}]", head.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::schema::Schema;

    /// Every keyword this module acts on. Anything outside this set is an
    /// annotation as far as the evaluator is concerned.
    const HANDLED: &[&str] = &[
        // reference
        "$ref",
        "$dynamicRef",
        "$id",
        "$anchor",
        "$dynamicAnchor",
        "$defs",
        "$schema",
        "$vocabulary",
        // any instance
        "type",
        "enum",
        "const",
        // numbers
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "multipleOf",
        // strings
        "minLength",
        "maxLength",
        "pattern",
        // arrays
        "prefixItems",
        "items",
        "contains",
        "minContains",
        "maxContains",
        "minItems",
        "maxItems",
        "uniqueItems",
        // objects
        "properties",
        "patternProperties",
        "additionalProperties",
        "propertyNames",
        "required",
        "dependentRequired",
        "dependentSchemas",
        "minProperties",
        "maxProperties",
        // applicators
        "allOf",
        "anyOf",
        "oneOf",
        "not",
        "if",
        "then",
        "else",
        // annotations, deliberately inert
        "title",
        "description",
        "default",
        "examples",
        "deprecated",
        "readOnly",
        "writeOnly",
        "$comment",
        "format",
        "contentEncoding",
        "contentMediaType",
        "contentSchema",
    ];

    /// The guard promised in this module's docs: if a future respin of the
    /// vendored meta-schema reaches for a keyword the evaluator ignores, this
    /// fails rather than letting validation quietly go soft.
    #[test]
    fn the_meta_schema_uses_no_keyword_this_module_ignores() {
        fn collect(node: &Value, found: &mut BTreeSet<String>) {
            let Some(object) = node.as_object() else {
                return;
            };
            for (key, value) in object {
                found.insert(key.clone());
                match key.as_str() {
                    "properties" | "patternProperties" | "$defs" | "dependentSchemas" => {
                        for sub in value.as_object().into_iter().flatten().map(|(_, v)| v) {
                            collect(sub, found);
                        }
                    }
                    "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                        for sub in value.as_array().into_iter().flatten() {
                            collect(sub, found);
                        }
                    }
                    "items"
                    | "not"
                    | "if"
                    | "then"
                    | "else"
                    | "additionalProperties"
                    | "propertyNames"
                    | "contains"
                    | "contentSchema" => collect(value, found),
                    _ => {}
                }
            }
        }

        let meta = MetaSchema::current();
        let mut found = BTreeSet::new();
        for document in crate::meta::DOCUMENTS {
            collect(meta.document(document.uri).unwrap(), &mut found);
        }

        let handled: BTreeSet<&str> = HANDLED.iter().copied().collect();
        let ignored: Vec<&String> = found
            .iter()
            .filter(|k| !handled.contains(k.as_str()))
            .collect();
        assert!(
            ignored.is_empty(),
            "the meta-schema uses unhandled keywords: {ignored:?}"
        );
    }



    #[test]
    fn json_equality_compares_numbers_by_value() {
        assert!(json_eq(&serde_json::json!(1), &serde_json::json!(1.0)));
        assert!(json_eq(
            &serde_json::json!([1, {"a": 2}]),
            &serde_json::json!([1.0, {"a": 2.0}])
        ));
        assert!(!json_eq(&serde_json::json!(1), &serde_json::json!("1")));
        assert!(!json_eq(
            &serde_json::json!({"a": 1}),
            &serde_json::json!({"a": 1, "b": 2})
        ));
    }

    #[test]
    fn integer_accepts_a_zero_fraction_and_rejects_a_real_one() {
        assert!(type_matches("integer", &serde_json::json!(1.0)));
        assert!(!type_matches("integer", &serde_json::json!(1.5)));
        assert!(type_matches("number", &serde_json::json!(1.5)));
        assert!(!type_matches("number", &serde_json::json!("1")));
    }

    #[test]
    fn duplicates_are_found_under_json_equality() {
        let items = vec![
            serde_json::json!(1),
            serde_json::json!("a"),
            serde_json::json!(1.0),
        ];
        assert_eq!(first_duplicate(&items), Some(&serde_json::json!(1.0)));
        assert_eq!(first_duplicate(&items[..2]), None);
    }

    #[test]
    fn a_well_formed_schema_passes() {
        let document = Schema::object()
            .dialect()
            .id("https://example.com/x")
            .required_property("a", Schema::string().min_length(1))
            .build_unchecked()
            .to_value();
        assert_eq!(validate_document(&document), Ok(()));
    }
}

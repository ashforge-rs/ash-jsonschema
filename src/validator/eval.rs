//! The evaluator: does this instance satisfy this schema?
//!
//! Separate from [`crate::validate`], which checks that a *schema* is
//! well-formed against the meta-schema. That one is deliberately narrow — its
//! schema side never varies — and this one has to cope with whatever schema a
//! caller hands it.
//!
//! ## Annotations, and why they are here
//!
//! `unevaluatedProperties` and `unevaluatedItems` do not ask what *this*
//! schema matched; they ask what everything applied to this instance location
//! matched, including through `$ref`, `allOf`, `anyOf`, `if`/`then`, and
//! whatever those reached in turn. That is the annotation collection the 2020-12
//! specification describes, and it is why the evaluator returns a set of
//! evaluated positions alongside its verdict rather than a bare bool.
//!
//! The rule that makes it subtle: a subschema that **fails** contributes no
//! annotations. A failed `anyOf` branch evaluates nothing, so its properties
//! stay unevaluated and `unevaluatedProperties: false` still rejects them.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::meta::escape_pointer_token;
use crate::uri;
use crate::validator::compile::Compiled;
use crate::validator::outcome::{Mode, Outcome, ValidationError, one_line};

/// How deep the evaluator will recurse before giving up.
///
/// A `$ref` cycle through a schema that never consumes instance — the classic
/// `{"$ref": "#"}` — would otherwise recurse until the stack gives out, which
/// aborts the process uncatchably. Hitting this reports an ordinary error
/// instead.
pub const MAX_DEPTH: usize = 128;

/// What a subschema evaluated at one instance location.
///
/// Collected so `unevaluatedProperties` and `unevaluatedItems` can ask what is
/// left over. Only positions evaluated by a *successful* subschema count.
#[derive(Clone, Debug, Default)]
pub(crate) struct Evaluated {
    /// Property names of the object at this location that some schema applied
    /// to.
    pub properties: BTreeSet<String>,
    /// The array indices some schema applied to.
    ///
    /// A set rather than a high-water mark, because `contains` evaluates
    /// whichever items happen to match — possibly index 2 while leaving index 1
    /// untouched — and `unevaluatedItems` must still see the gap.
    pub items: BTreeSet<usize>,
    /// Whether a schema evaluated *every* item, as `items` does.
    pub all_items: bool,
}

impl Evaluated {
    /// Merges what another subschema evaluated into this.
    fn absorb(&mut self, other: Evaluated) {
        self.properties.extend(other.properties);
        self.items.extend(other.items);
        self.all_items |= other.all_items;
    }

    /// Whether the item at `index` was evaluated by anything.
    fn covers(&self, index: usize) -> bool {
        self.all_items || self.items.contains(&index)
    }
}

pub(crate) struct Evaluator<'c> {
    pub(crate) compiled: &'c Compiled,
    pub(crate) mode: Mode,
    pub(crate) errors: Vec<ValidationError>,
    /// Non-zero while inside a branch whose failure is not itself an error —
    /// an `anyOf` alternative, an `if` condition, the subject of a `not`.
    pub(crate) silent: usize,
    pub(crate) depth: usize,
}

/// Where a subschema is being applied: the schema side and the instance side.
#[derive(Clone, Copy)]
struct Where<'a> {
    /// JSON Pointer into the schema.
    keyword: &'a str,
    /// JSON Pointer into the instance.
    location: &'a str,
    /// The base URI in force, which a relative `$ref` resolves against.
    base: &'a str,
    /// Every resource entered on the way here, outermost first.
    ///
    /// `$dynamicRef` searches this rather than the schema as written: which
    /// `$dynamicAnchor` it lands on depends on the path the evaluation actually
    /// took, which is the whole point of the keyword and the reason the scope
    /// travels with the evaluation instead of being indexed up front.
    dynamic: &'a [String],
}

impl Evaluator<'_> {
    /// Whether enough has gone wrong to stop.
    fn done(&self) -> bool {
        self.mode == Mode::FailFast && !self.errors.is_empty() && self.silent == 0
    }

    fn report(&mut self, at: Where<'_>, keyword: &str, message: impl Into<String>) {
        if self.silent > 0 {
            return;
        }
        self.errors.push(ValidationError {
            location: at.location.to_owned(),
            keyword: match keyword.is_empty() {
                true => at.keyword.to_owned(),
                false => format!("{}/{keyword}", at.keyword),
            },
            message: message.into(),
        });
    }

    /// Evaluates `schema` against `instance`, returning whether it matched and
    /// what it evaluated.
    fn eval(
        &mut self,
        schema: &Value,
        instance: &Value,
        at: Where<'_>,
    ) -> (bool, Evaluated) {
        let map = match schema {
            Value::Bool(true) => return (true, Evaluated::default()),
            Value::Bool(false) => {
                // A `false` schema rejects everything. Where it sits under
                // `additionalProperties` or `unevaluatedProperties` — by far
                // the commonest place — that means the property is not allowed
                // at all, which is worth saying plainly.
                let message = match at.keyword.rsplit('/').next() {
                    Some("additionalProperties" | "unevaluatedProperties") => {
                        "is not a permitted property"
                    }
                    Some("items" | "unevaluatedItems" | "prefixItems") => {
                        "is not a permitted item"
                    }
                    _ => "no value is valid here",
                };
                self.report(at, "", message);
                return (false, Evaluated::default());
            }
            Value::Object(map) => map,
            // `Compiled` checked the schema is well formed, so this cannot
            // happen for a schema that got that far.
            _ => {
                self.report(at, "", "not a schema");
                return (false, Evaluated::default());
            }
        };

        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            self.report(
                at,
                "",
                format!("nested too deeply to check (past {MAX_DEPTH} steps)"),
            );
            return (false, Evaluated::default());
        }

        // A nested `$id` rebases the references beneath it, and joins the
        // dynamic scope that `$dynamicRef` searches.
        let base;
        let entered;
        let at = match map.get("$id").and_then(Value::as_str) {
            Some(id) => {
                base = uri::resolve(at.base, id).0;
                entered = push_scope(at.dynamic, &base);
                Where {
                    base: &base,
                    dynamic: &entered,
                    ..at
                }
            }
            None => at,
        };

        let result = self.eval_keywords(map, instance, at);
        self.depth -= 1;
        result
    }

    fn eval_keywords(
        &mut self,
        map: &Map<String, Value>,
        instance: &Value,
        at: Where<'_>,
    ) -> (bool, Evaluated) {
        let mut valid = true;
        let mut evaluated = Evaluated::default();

        // --- references ---------------------------------------------------
        // In 2020-12 `$ref` sits alongside its siblings rather than replacing
        // them, so what it evaluates joins theirs.
        for keyword in ["$ref", "$dynamicRef"] {
            let Some(target) = map.get(keyword).and_then(Value::as_str) else {
                continue;
            };
            let dynamic = keyword == "$dynamicRef";
            match self
                .compiled
                .resolve_in_scope(at.base, target, dynamic, at.dynamic)
            {
                Some((resolved, resolved_base)) => {
                    let scope = push_scope(at.dynamic, &resolved_base);
                    let inner = Where {
                        keyword: &format!("{}/{keyword}", at.keyword),
                        location: at.location,
                        base: &resolved_base,
                        dynamic: &scope,
                    };
                    let (ok, seen) = self.eval(&resolved, instance, inner);
                    valid &= ok;
                    if ok {
                        evaluated.absorb(seen);
                    }
                }
                None => self.report(at, keyword, format!("`{target}` resolves to nothing")),
            }
            if self.done() {
                return (valid, evaluated);
            }
        }

        // --- assertions -----------------------------------------------------
        valid &= self.check_type(map, instance, at);
        if self.done() {
            return (valid, evaluated);
        }
        valid &= self.check_enum(map, instance, at);
        valid &= self.check_number(map, instance, at);
        valid &= self.check_string(map, instance, at);
        if self.done() {
            return (valid, evaluated);
        }

        let (array_ok, array_seen) = self.check_array(map, instance, at);
        valid &= array_ok;
        evaluated.absorb(array_seen);
        if self.done() {
            return (valid, evaluated);
        }

        let (object_ok, object_seen) = self.check_object(map, instance, at);
        valid &= object_ok;
        evaluated.absorb(object_seen);
        if self.done() {
            return (valid, evaluated);
        }

        // --- in-place applicators -------------------------------------------
        let (applied_ok, applied_seen) = self.check_applicators(map, instance, at);
        valid &= applied_ok;
        evaluated.absorb(applied_seen);
        if self.done() {
            return (valid, evaluated);
        }

        // --- unevaluated ----------------------------------------------------
        // These run last, and only they see the whole picture: everything the
        // keywords above evaluated, at this instance location, including
        // through references and applicators.
        valid &= self.check_unevaluated(map, instance, at, &mut evaluated);

        (valid, evaluated)
    }

    // --- assertions -------------------------------------------------------

    fn check_type(&mut self, map: &Map<String, Value>, instance: &Value, at: Where<'_>) -> bool {
        let Some(types) = map.get("type") else {
            return true;
        };
        let matches = match types {
            Value::String(one) => type_matches(one, instance),
            Value::Array(many) => many
                .iter()
                .filter_map(Value::as_str)
                .any(|one| type_matches(one, instance)),
            _ => true,
        };
        if !matches {
            let wanted = match types {
                Value::String(one) => one.clone(),
                other => other.to_string(),
            };
            self.report(
                at,
                "type",
                format!("expected {wanted}, found {}", type_of(instance)),
            );
        }
        matches
    }

    fn check_enum(&mut self, map: &Map<String, Value>, instance: &Value, at: Where<'_>) -> bool {
        let mut valid = true;

        // `map.get` is the test for presence: `const: null` is a real
        // assertion, and `is_null()` would read it as an absent keyword.
        if let Some(constant) = map.get("const")
            && !json_eq(constant, instance)
        {
            self.report(at, "const", format!("must be {}", one_line(constant)));
            valid = false;
        }
        if let Some(Value::Array(allowed)) = map.get("enum")
            && !allowed.iter().any(|value| json_eq(value, instance))
        {
            let rendered: Vec<String> = allowed.iter().take(6).map(one_line).collect();
            let ellipsis = match allowed.len() > 6 {
                true => ", …",
                false => "",
            };
            self.report(
                at,
                "enum",
                format!("must be one of [{}{ellipsis}]", rendered.join(", ")),
            );
            valid = false;
        }
        valid
    }

    fn check_number(&mut self, map: &Map<String, Value>, instance: &Value, at: Where<'_>) -> bool {
        let Some(number) = instance.as_f64() else {
            return true;
        };
        let mut valid = true;

        let bounds: [NumericBound; 4] = [
            ("minimum", |value, bound| value >= bound, "at least"),
            ("maximum", |value, bound| value <= bound, "at most"),
            (
                "exclusiveMinimum",
                |value, bound| value > bound,
                "greater than",
            ),
            (
                "exclusiveMaximum",
                |value, bound| value < bound,
                "less than",
            ),
        ];
        for (keyword, holds, describe) in bounds {
            if let Some(bound) = map.get(keyword).and_then(Value::as_f64)
                && !holds(number, bound)
            {
                self.report(at, keyword, format!("must be {describe} {}", trim(bound)));
                valid = false;
            }
        }

        if let Some(divisor) = map.get("multipleOf").and_then(Value::as_f64)
            && divisor > 0.0
        {
            let quotient = number / divisor;
            // A divisor small enough to overflow the quotient cannot divide the
            // value evenly — the specification's own suite has a case for it,
            // since a naive implementation raises here instead of answering.
            let divides = quotient.is_finite()
                && (quotient - quotient.round()).abs() <= f64::EPSILON * quotient.abs().max(1.0);
            if !divides {
                self.report(
                    at,
                    "multipleOf",
                    format!("must be a multiple of {}", trim(divisor)),
                );
                valid = false;
            }
        }
        valid
    }

    fn check_string(&mut self, map: &Map<String, Value>, instance: &Value, at: Where<'_>) -> bool {
        let Some(text) = instance.as_str() else {
            return true;
        };
        let mut valid = true;
        // The specification counts code points, not bytes or UTF-16 units.
        let length = text.chars().count() as u64;

        if let Some(min) = map.get("minLength").and_then(as_count)
            && length < min
        {
            self.report(
                at,
                "minLength",
                format!("must be at least {}", plural(min, "character")),
            );
            valid = false;
        }
        if let Some(max) = map.get("maxLength").and_then(as_count)
            && length > max
        {
            self.report(
                at,
                "maxLength",
                format!("must be at most {}", plural(max, "character")),
            );
            valid = false;
        }
        if let Some(pattern) = map.get("pattern").and_then(Value::as_str)
            && let Some(regex) = self.compiled.regex(pattern)
            && !regex.is_match(text)
        {
            self.report(at, "pattern", format!("must match /{pattern}/"));
            valid = false;
        }
        valid
    }

    fn check_array(
        &mut self,
        map: &Map<String, Value>,
        instance: &Value,
        at: Where<'_>,
    ) -> (bool, Evaluated) {
        let mut evaluated = Evaluated::default();
        let Some(items) = instance.as_array() else {
            return (true, evaluated);
        };
        let mut valid = true;

        if let Some(min) = map.get("minItems").and_then(as_count)
            && (items.len() as u64) < min
        {
            self.report(
                at,
                "minItems",
                format!("must have at least {}", plural(min, "item")),
            );
            valid = false;
        }
        if let Some(max) = map.get("maxItems").and_then(as_count)
            && (items.len() as u64) > max
        {
            self.report(
                at,
                "maxItems",
                format!("must have at most {}", plural(max, "item")),
            );
            valid = false;
        }
        if map.get("uniqueItems") == Some(&Value::Bool(true))
            && let Some(duplicate) = first_duplicate(items)
        {
            self.report(
                at,
                "uniqueItems",
                format!("items must be unique, {} is repeated", one_line(duplicate)),
            );
            valid = false;
        }

        let mut covered = 0;
        if let Some(Value::Array(prefix)) = map.get("prefixItems") {
            covered = prefix.len().min(items.len());
            for (index, schema) in prefix.iter().take(covered).enumerate() {
                let keyword = format!("{}/prefixItems/{index}", at.keyword);
                let location = index_location(at.location, index);
                let inner = Where {
                    keyword: &keyword,
                    location: &location,
                    base: at.base,
                    dynamic: at.dynamic,
                };
                let (ok, _) = self.eval(schema, &items[index], inner);
                valid &= ok;
                if self.done() {
                    return (valid, evaluated);
                }
            }
            evaluated.items.extend(0..covered);
        }

        if let Some(schema) = map.get("items") {
            let keyword = format!("{}/items", at.keyword);
            for (index, item) in items.iter().enumerate().skip(covered) {
                let location = index_location(at.location, index);
                let inner = Where {
                    keyword: &keyword,
                    location: &location,
                    base: at.base,
                    dynamic: at.dynamic,
                };
                let (ok, _) = self.eval(schema, item, inner);
                valid &= ok;
                if self.done() {
                    return (valid, evaluated);
                }
            }
            // `items` applies to everything past the prefix, so the whole array
            // counts as evaluated even when it is empty.
            evaluated.items.extend(0..items.len());
            evaluated.all_items = true;
        }

        if let Some(schema) = map.get("contains") {
            let keyword = format!("{}/contains", at.keyword);
            let mut matched = 0u64;
            for (index, item) in items.iter().enumerate() {
                let location = index_location(at.location, index);
                let inner = Where {
                    keyword: &keyword,
                    location: &location,
                    base: at.base,
                    dynamic: at.dynamic,
                };
                self.silent += 1;
                let (ok, _) = self.eval(schema, item, inner);
                self.silent -= 1;
                if ok {
                    matched += 1;
                    // Only the items that *match* are evaluated, and they need
                    // not be contiguous — recording a high-water mark here
                    // would wrongly cover the ones in between.
                    evaluated.items.insert(index);
                }
            }

            let min = map.get("minContains").and_then(as_count).unwrap_or(1);
            let max = map
                .get("maxContains")
                .and_then(as_count)
                .unwrap_or(u64::MAX);
            if matched < min || matched > max {
                let expected = match max {
                    u64::MAX => format!("at least {min}"),
                    _ => format!("between {min} and {max}"),
                };
                self.report(
                    at,
                    "contains",
                    format!("{matched} items match `contains`, expected {expected}"),
                );
                valid = false;
            }
        }

        (valid, evaluated)
    }

    fn check_object(
        &mut self,
        map: &Map<String, Value>,
        instance: &Value,
        at: Where<'_>,
    ) -> (bool, Evaluated) {
        let mut evaluated = Evaluated::default();
        let Some(object) = instance.as_object() else {
            return (true, evaluated);
        };
        let mut valid = true;

        if let Some(min) = map.get("minProperties").and_then(as_count)
            && (object.len() as u64) < min
        {
            self.report(
                at,
                "minProperties",
                format!("must have at least {min} properties"),
            );
            valid = false;
        }
        if let Some(max) = map.get("maxProperties").and_then(as_count)
            && (object.len() as u64) > max
        {
            self.report(
                at,
                "maxProperties",
                format!("must have at most {max} properties"),
            );
            valid = false;
        }

        if let Some(Value::Array(names)) = map.get("required") {
            for name in names.iter().filter_map(Value::as_str) {
                if !object.contains_key(name) {
                    self.report(at, "required", format!("`{name}` is required"));
                    valid = false;
                    if self.done() {
                        return (valid, evaluated);
                    }
                }
            }
        }

        if let Some(Value::Object(dependencies)) = map.get("dependentRequired") {
            for (trigger, names) in dependencies {
                if !object.contains_key(trigger) {
                    continue;
                }
                let Some(names) = names.as_array() else { continue };
                for name in names.iter().filter_map(Value::as_str) {
                    if !object.contains_key(name) {
                        self.report(
                            at,
                            "dependentRequired",
                            format!("`{name}` is required when `{trigger}` is present"),
                        );
                        valid = false;
                    }
                }
            }
            if self.done() {
                return (valid, evaluated);
            }
        }

        if let Some(Value::Object(properties)) = map.get("properties") {
            for (name, schema) in properties {
                let Some(value) = object.get(name) else {
                    continue;
                };
                let token = escape_pointer_token(name);
                let keyword = format!("{}/properties/{token}", at.keyword);
                let location = format!("{}/{token}", at.location);
                let inner = Where {
                    keyword: &keyword,
                    location: &location,
                    base: at.base,
                    dynamic: at.dynamic,
                };
                let (ok, _) = self.eval(schema, value, inner);
                valid &= ok;
                evaluated.properties.insert(name.clone());
                if self.done() {
                    return (valid, evaluated);
                }
            }
        }

        if let Some(Value::Object(patterns)) = map.get("patternProperties") {
            for (pattern, schema) in patterns {
                let Some(regex) = self.compiled.regex(pattern) else {
                    continue;
                };
                let names: Vec<&String> = object
                    .keys()
                    .filter(|name| regex.is_match(name))
                    .collect();
                for name in names {
                    let token = escape_pointer_token(name);
                    let keyword =
                        format!("{}/patternProperties/{}", at.keyword, escape_pointer_token(pattern));
                    let location = format!("{}/{token}", at.location);
                    let inner = Where {
                        keyword: &keyword,
                        location: &location,
                        base: at.base,
                    dynamic: at.dynamic,
                    };
                    let (ok, _) = self.eval(schema, &object[name], inner);
                    valid &= ok;
                    evaluated.properties.insert(name.clone());
                    if self.done() {
                        return (valid, evaluated);
                    }
                }
            }
        }

        if let Some(schema) = map.get("additionalProperties") {
            let keyword = format!("{}/additionalProperties", at.keyword);
            let leftover: Vec<&String> = object
                .keys()
                .filter(|name| !evaluated.properties.contains(*name))
                .collect();
            for name in leftover {
                let token = escape_pointer_token(name);
                let location = format!("{}/{token}", at.location);
                let inner = Where {
                    keyword: &keyword,
                    location: &location,
                    base: at.base,
                    dynamic: at.dynamic,
                };
                let (ok, _) = self.eval(schema, &object[name], inner);
                valid &= ok;
                evaluated.properties.insert(name.clone());
                if self.done() {
                    return (valid, evaluated);
                }
            }
        }

        if let Some(schema) = map.get("propertyNames") {
            let keyword = format!("{}/propertyNames", at.keyword);
            for name in object.keys() {
                let token = escape_pointer_token(name);
                let location = format!("{}/{token}", at.location);
                let inner = Where {
                    keyword: &keyword,
                    location: &location,
                    base: at.base,
                    dynamic: at.dynamic,
                };
                // The instance handed to `propertyNames` is the name itself.
                let (ok, _) = self.eval(schema, &Value::String(name.clone()), inner);
                valid &= ok;
                if self.done() {
                    return (valid, evaluated);
                }
            }
        }

        if let Some(Value::Object(dependencies)) = map.get("dependentSchemas") {
            for (trigger, schema) in dependencies {
                if !object.contains_key(trigger) {
                    continue;
                }
                let keyword = format!(
                    "{}/dependentSchemas/{}",
                    at.keyword,
                    escape_pointer_token(trigger)
                );
                let inner = Where {
                    keyword: &keyword,
                    location: at.location,
                    base: at.base,
                    dynamic: at.dynamic,
                };
                let (ok, seen) = self.eval(schema, instance, inner);
                valid &= ok;
                if ok {
                    evaluated.absorb(seen);
                }
                if self.done() {
                    return (valid, evaluated);
                }
            }
        }

        (valid, evaluated)
    }

    fn check_applicators(
        &mut self,
        map: &Map<String, Value>,
        instance: &Value,
        at: Where<'_>,
    ) -> (bool, Evaluated) {
        let mut evaluated = Evaluated::default();
        let mut valid = true;

        if let Some(Value::Array(branches)) = map.get("allOf") {
            for (index, schema) in branches.iter().enumerate() {
                let keyword = format!("{}/allOf/{index}", at.keyword);
                let inner = Where {
                    keyword: &keyword,
                    location: at.location,
                    base: at.base,
                    dynamic: at.dynamic,
                };
                let (ok, seen) = self.eval(schema, instance, inner);
                valid &= ok;
                if ok {
                    evaluated.absorb(seen);
                }
                if self.done() {
                    return (valid, evaluated);
                }
            }
        }

        if let Some(Value::Array(branches)) = map.get("anyOf") {
            let mut matched = false;
            for (index, schema) in branches.iter().enumerate() {
                let keyword = format!("{}/anyOf/{index}", at.keyword);
                let inner = Where {
                    keyword: &keyword,
                    location: at.location,
                    base: at.base,
                    dynamic: at.dynamic,
                };
                // A branch that fails is not an error — only all of them
                // failing is — so its errors are suppressed.
                self.silent += 1;
                let (ok, seen) = self.eval(schema, instance, inner);
                self.silent -= 1;
                if ok {
                    matched = true;
                    // Only a *successful* branch contributes annotations.
                    evaluated.absorb(seen);
                }
            }
            if !matched {
                self.report(at, "anyOf", "does not match any permitted alternative");
                valid = false;
                if self.done() {
                    return (valid, evaluated);
                }
            }
        }

        if let Some(Value::Array(branches)) = map.get("oneOf") {
            let mut matches = Vec::new();
            let mut winner = Evaluated::default();
            for (index, schema) in branches.iter().enumerate() {
                let keyword = format!("{}/oneOf/{index}", at.keyword);
                let inner = Where {
                    keyword: &keyword,
                    location: at.location,
                    base: at.base,
                    dynamic: at.dynamic,
                };
                self.silent += 1;
                let (ok, seen) = self.eval(schema, instance, inner);
                self.silent -= 1;
                if ok {
                    matches.push(index);
                    winner = seen;
                }
            }
            match matches.len() {
                1 => evaluated.absorb(winner),
                0 => {
                    self.report(at, "oneOf", "does not match any permitted alternative");
                    valid = false;
                }
                count => {
                    let listed: Vec<String> =
                        matches.iter().map(|index| index.to_string()).collect();
                    self.report(
                        at,
                        "oneOf",
                        format!(
                            "must match exactly one alternative, matched {count} ({})",
                            listed.join(", "),
                        ),
                    );
                    valid = false;
                }
            }
            if self.done() {
                return (valid, evaluated);
            }
        }

        if let Some(schema) = map.get("not") {
            let keyword = format!("{}/not", at.keyword);
            let inner = Where {
                keyword: &keyword,
                location: at.location,
                base: at.base,
                    dynamic: at.dynamic,
            };
            self.silent += 1;
            let (ok, _) = self.eval(schema, instance, inner);
            self.silent -= 1;
            if ok {
                self.report(at, "not", "must not match, but does");
                valid = false;
                if self.done() {
                    return (valid, evaluated);
                }
            }
        }

        // `if` asserts nothing itself: it only selects between `then` and
        // `else`, and its own annotations count when it matched.
        if let Some(condition) = map.get("if") {
            let keyword = format!("{}/if", at.keyword);
            let inner = Where {
                keyword: &keyword,
                location: at.location,
                base: at.base,
                    dynamic: at.dynamic,
            };
            self.silent += 1;
            let (matched, seen) = self.eval(condition, instance, inner);
            self.silent -= 1;

            let branch = match matched {
                true => {
                    evaluated.absorb(seen);
                    "then"
                }
                false => "else",
            };
            if let Some(schema) = map.get(branch) {
                let keyword = format!("{}/{branch}", at.keyword);
                let inner = Where {
                    keyword: &keyword,
                    location: at.location,
                    base: at.base,
                    dynamic: at.dynamic,
                };
                let (ok, seen) = self.eval(schema, instance, inner);
                valid &= ok;
                if ok {
                    evaluated.absorb(seen);
                }
            }
        }

        (valid, evaluated)
    }

    /// `unevaluatedProperties` and `unevaluatedItems`, which see everything the
    /// other keywords evaluated at this location.
    fn check_unevaluated(
        &mut self,
        map: &Map<String, Value>,
        instance: &Value,
        at: Where<'_>,
        evaluated: &mut Evaluated,
    ) -> bool {
        let mut valid = true;

        if let Some(schema) = map.get("unevaluatedProperties")
            && let Some(object) = instance.as_object()
        {
            let keyword = format!("{}/unevaluatedProperties", at.keyword);
            let leftover: Vec<String> = object
                .keys()
                .filter(|name| !evaluated.properties.contains(*name))
                .cloned()
                .collect();
            for name in leftover {
                let token = escape_pointer_token(&name);
                let location = format!("{}/{token}", at.location);
                let inner = Where {
                    keyword: &keyword,
                    location: &location,
                    base: at.base,
                    dynamic: at.dynamic,
                };
                let (ok, _) = self.eval(schema, &object[&name], inner);
                valid &= ok;
                evaluated.properties.insert(name);
                if self.done() {
                    return valid;
                }
            }
        }

        if let Some(schema) = map.get("unevaluatedItems")
            && let Some(items) = instance.as_array()
        {
            let keyword = format!("{}/unevaluatedItems", at.keyword);
            for (index, item) in items.iter().enumerate() {
                if evaluated.covers(index) {
                    continue;
                }
                let location = index_location(at.location, index);
                let inner = Where {
                    keyword: &keyword,
                    location: &location,
                    base: at.base,
                    dynamic: at.dynamic,
                };
                let (ok, _) = self.eval(schema, item, inner);
                valid &= ok;
                if self.done() {
                    return valid;
                }
            }
            evaluated.items.extend(0..items.len());
            evaluated.all_items = true;
        }

        valid
    }
}

/// Runs a compiled schema over an instance.
pub(crate) fn run(compiled: &Compiled, instance: &Value, mode: Mode) -> Outcome {
    let mut evaluator = Evaluator {
        compiled,
        mode,
        errors: Vec::new(),
        silent: 0,
        depth: 0,
    };
    let root = compiled.root();
    let scope = [compiled.base().to_owned()];
    let at = Where {
        keyword: "",
        location: "",
        base: compiled.base(),
        dynamic: &scope,
    };
    evaluator.eval(root, instance, at);

    Outcome {
        errors: evaluator.errors,
    }
}

// --- helpers --------------------------------------------------------------

/// One numeric bound: its keyword, the test it makes, and how to word a
/// failure.
type NumericBound = (&'static str, fn(f64, f64) -> bool, &'static str);

/// `1 item` but `2 items`.
fn plural(count: u64, singular: &str) -> String {
    match count {
        1 => format!("{count} {singular}"),
        _ => format!("{count} {singular}s"),
    }
}

/// Extends a dynamic scope with a resource, if it is not already in it.
fn push_scope(scope: &[String], resource: &str) -> Vec<String> {
    let mut extended = scope.to_vec();
    if !extended.iter().any(|entered| entered == resource) {
        extended.push(resource.to_owned());
    }
    extended
}

/// The `type` name for an instance, as the specification writes it.
fn type_of(instance: &Value) -> &'static str {
    match instance {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Whether an instance is of the named type.
///
/// `integer` is the one that is not simply the JSON type: a number with a zero
/// fractional part qualifies, so `1.0` is an integer and `1.5` is not.
fn type_matches(name: &str, instance: &Value) -> bool {
    match name {
        "integer" => match instance {
            Value::Number(number) => number
                .as_f64()
                .is_some_and(|value| value.fract() == 0.0 && value.is_finite()),
            _ => false,
        },
        other => other == type_of(instance),
    }
}

/// JSON equality, which compares numbers by value rather than by
/// representation — `1` and `1.0` are the same instance.
fn json_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => match (left.as_f64(), right.as_f64()) {
            (Some(left), Some(right)) => left == right,
            _ => left == right,
        },
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| json_eq(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right.get(key).is_some_and(|other| json_eq(value, other))
                })
        }
        _ => left == right,
    }
}

/// The first value that appears more than once, under JSON equality.
fn first_duplicate(items: &[Value]) -> Option<&Value> {
    items.iter().enumerate().find_map(|(index, item)| {
        items[..index]
            .iter()
            .any(|earlier| json_eq(earlier, item))
            .then_some(item)
    })
}

/// A count keyword's value.
///
/// The meta-schema types these as non-negative integers, which permits `2.0` as
/// readily as `2` — JSON draws no distinction. `as_u64` refuses the decimal
/// form, so it cannot be used on its own or a bound written that way silently
/// stops applying.
fn as_count(value: &Value) -> Option<u64> {
    if let Some(count) = value.as_u64() {
        return Some(count);
    }
    let number = value.as_f64()?;
    (number.fract() == 0.0 && number >= 0.0 && number <= u64::MAX as f64)
        .then_some(number as u64)
}

/// A pointer into an array element.
fn index_location(location: &str, index: usize) -> String {
    format!("{location}/{index}")
}

/// Renders a bound without a trailing `.0` on a whole number.
fn trim(value: f64) -> String {
    match value.fract() == 0.0 && value.abs() < 1e15 {
        true => format!("{}", value as i64),
        false => format!("{value}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_accepts_a_whole_float_and_rejects_a_fractional_one() {
        assert!(type_matches("integer", &serde_json::json!(1)));
        assert!(type_matches("integer", &serde_json::json!(1.0)));
        assert!(!type_matches("integer", &serde_json::json!(1.5)));
        assert!(!type_matches("integer", &serde_json::json!("1")));
    }

    #[test]
    fn number_accepts_both_but_string_accepts_neither() {
        assert!(type_matches("number", &serde_json::json!(1.5)));
        assert!(type_matches("number", &serde_json::json!(1)));
        assert!(!type_matches("number", &serde_json::json!("1")));
        assert!(type_matches("string", &serde_json::json!("1")));
    }

    #[test]
    fn json_equality_compares_numbers_by_value() {
        assert!(json_eq(&serde_json::json!(1), &serde_json::json!(1.0)));
        assert!(json_eq(
            &serde_json::json!({"a": [1]}),
            &serde_json::json!({"a": [1.0]}),
        ));
        assert!(!json_eq(&serde_json::json!(1), &serde_json::json!("1")));
        assert!(!json_eq(
            &serde_json::json!({"a": 1}),
            &serde_json::json!({"a": 1, "b": 2}),
        ));
    }

    #[test]
    fn a_duplicate_is_found_by_value_not_representation() {
        let items = vec![
            serde_json::json!(1),
            serde_json::json!(2),
            serde_json::json!(1.0),
        ];
        assert_eq!(first_duplicate(&items), Some(&serde_json::json!(1.0)));

        let unique = vec![serde_json::json!(1), serde_json::json!(2)];
        assert_eq!(first_duplicate(&unique), None);
    }

    #[test]
    fn a_whole_bound_renders_without_a_fraction() {
        assert_eq!(trim(3.0), "3");
        assert_eq!(trim(-3.0), "-3");
        assert_eq!(trim(2.5), "2.5");
    }

    #[test]
    fn absorbing_takes_the_union_of_both_sides() {
        let mut left = Evaluated {
            properties: ["a".to_owned()].into_iter().collect(),
            items: [0, 2].into_iter().collect(),
            all_items: false,
        };
        left.absorb(Evaluated {
            properties: ["b".to_owned()].into_iter().collect(),
            items: [5].into_iter().collect(),
            all_items: true,
        });

        assert_eq!(left.properties.len(), 2);
        // A union, not a high-water mark: index 1 stays uncovered.
        assert!(left.items.contains(&0) && left.items.contains(&5));
        assert!(left.all_items);
    }
}

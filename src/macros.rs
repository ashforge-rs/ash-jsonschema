//! Macros for the shapes that are wordy to write out.
//!
//! These exist where the builder makes you repeat *structure* — a list of
//! properties, a set of definitions — not to shorten a single keyword. There is
//! no macro for `.min_length(1)`, because a macro that wraps one method call
//! only adds a second thing to learn.
//!
//! Everything a macro writes is an ordinary builder chain, so the two mix
//! freely: a macro can build a subschema inside a hand-written chain, and a
//! hand-written chain can sit inside a macro.

/// An object schema, written the way its instances look.
///
/// A property is optional by default and required when marked with `required`.
/// That is the one distinction object schemas force you to spell out twice —
/// once in `properties`, once in `required` — and the whole reason this macro
/// earns its place.
///
/// ```
/// # use ash_jsonschema::{Schema, object};
/// let user = object! {
///     required "id": Schema::string().format("uuid"),
///     required "email": Schema::string().format("email"),
///     "age": Schema::integer().minimum(0),
///     "tags": Schema::array().items(Schema::string()),
/// }
/// .build()?;
///
/// assert_eq!(user.as_object().unwrap().required.as_ref().unwrap(), &["id", "email"]);
/// assert!(user.property("age").is_some());
/// # Ok::<(), ash_jsonschema::SchemaError>(())
/// ```
///
/// The result is an [`ObjectBuilder`](crate::ObjectBuilder), not a finished
/// schema, so the rest of the vocabulary is still there — and a nested `object!`
/// needs no `.build()`, exactly as a nested builder does not:
///
/// ```
/// # use ash_jsonschema::{Schema, object};
/// let schema = object! {
///     required "user": object! {
///         required "id": Schema::string(),
///     },
/// }
/// .title("Envelope")
/// .closed()
/// .build()?;
///
/// assert_eq!(schema.as_object().unwrap().title.as_deref(), Some("Envelope"));
/// # Ok::<(), ash_jsonschema::SchemaError>(())
/// ```
///
/// A property name is usually a literal, but any expression works when wrapped
/// in parentheses — for names that come from a config or a field list:
///
/// ```
/// # use ash_jsonschema::{Schema, object};
/// let tag = "x-request-id";
///
/// let schema = object! {
///     required (tag): Schema::string(),
///     (format!("x-{}", "trace")): Schema::string(),
/// }
/// .build()?;
///
/// assert!(schema.requires("x-request-id"));
/// assert!(schema.property("x-trace").is_some());
/// # Ok::<(), ash_jsonschema::SchemaError>(())
/// ```
///
/// An empty `object!{}` is `{"type": "object"}`, the same as
/// [`Schema::object`](crate::Schema::object).
#[macro_export]
macro_rules! object {
    ($($tail:tt)*) => {
        $crate::object_impl!($crate::Schema::object(); $($tail)*)
    };
}

/// The accumulator behind [`object!`]. Not part of the public API.
#[doc(hidden)]
#[macro_export]
macro_rules! object_impl {
    // Done.
    ($builder:expr;) => { $builder };
    ($builder:expr; ,) => { $builder };

    // `required <name>: schema`
    //
    // The name is matched as a parenthesised expression or a literal rather
    // than a bare `expr`, because an unparenthesised `expr` would swallow the
    // `:` that separates it from the schema.
    ($builder:expr; required ($name:expr) : $schema:expr $(, $($tail:tt)*)?) => {
        $crate::object_impl!($builder.required_property($name, $schema); $($($tail)*)?)
    };
    ($builder:expr; required $name:literal : $schema:expr $(, $($tail:tt)*)?) => {
        $crate::object_impl!($builder.required_property($name, $schema); $($($tail)*)?)
    };

    // `<name>: schema`
    ($builder:expr; ($name:expr) : $schema:expr $(, $($tail:tt)*)?) => {
        $crate::object_impl!($builder.property($name, $schema); $($($tail)*)?)
    };
    ($builder:expr; $name:literal : $schema:expr $(, $($tail:tt)*)?) => {
        $crate::object_impl!($builder.property($name, $schema); $($($tail)*)?)
    };
}

/// A set of named subschemas for `$defs`, applied to a schema.
///
/// `$defs` is where a document keeps the definitions its `$ref`s point at, and
/// writing several means repeating `.def(...)` for each. This takes them as a
/// block, and leaves the builder otherwise untouched.
///
/// ```
/// # use ash_jsonschema::{Schema, defs, object};
/// let schema = defs! {
///     Schema::object().id("https://example.com/order");
///
///     "Money": object! {
///         required "amount": Schema::integer().minimum(0),
///         required "currency": Schema::string().length(3),
///     },
///     "Sku": Schema::string().pattern("^[A-Z0-9-]+$"),
/// }
/// .required_property("total", Schema::reference("#/$defs/Money"))
/// .build()?;
///
/// assert!(schema.def("Money").is_some());
/// assert!(schema.def("Sku").is_some());
/// # Ok::<(), ash_jsonschema::SchemaError>(())
/// ```
///
/// The builder comes first, separated by `;`, so the macro reads as *this
/// schema, with these definitions* — and any builder works, whatever its type.
#[macro_export]
macro_rules! defs {
    ($builder:expr; $($name:tt : $schema:expr),* $(,)?) => {
        $builder$(.def($crate::defs_name!($name), $schema))*
    };
}

/// Unwraps a `defs!` name, which is a literal or a parenthesised expression.
/// Not part of the public API.
#[doc(hidden)]
#[macro_export]
macro_rules! defs_name {
    (($name:expr)) => { $name };
    ($name:literal) => { $name };
}

/// A `oneOf` over object schemas discriminated by a constant property — the
/// tagged union JSON Schema has no keyword for.
///
/// Spelling this out by hand means, for each variant, an object schema whose
/// tag property is pinned with `const` and marked required, all wrapped in a
/// `oneOf`. The shape is entirely mechanical and easy to get subtly wrong.
///
/// ```
/// # use ash_jsonschema::{Schema, one_of_tagged, object};
/// let shape = one_of_tagged! {
///     "kind";
///
///     "circle" => object! {
///         required "radius": Schema::number().exclusive_minimum(0),
///     },
///     "rect" => object! {
///         required "width": Schema::number(),
///         required "height": Schema::number(),
///     },
/// }
/// .build()?;
///
/// let branches = shape.as_object().unwrap().one_of.as_ref().unwrap();
/// assert_eq!(branches.len(), 2);
///
/// // Each branch pins the tag and requires it.
/// let circle = &branches[0];
/// assert!(circle.requires("kind"));
/// assert!(circle.requires("radius"));
/// # Ok::<(), ash_jsonschema::SchemaError>(())
/// ```
///
/// The tag property is added to each variant, so the variant bodies list only
/// what is theirs. The result is an [`AnyBuilder`](crate::AnyBuilder), ready for
/// a `$id`, a title, or anything else.
#[macro_export]
macro_rules! one_of_tagged {
    ($tag:literal; $($value:literal => $variant:expr),* $(,)?) => {
        $crate::Schema::one_of([
            $(
                $crate::Schema::from(
                    $variant.required_property(
                        $tag,
                        $crate::Schema::builder().constant($value),
                    )
                )
            ),*
        ])
    };
}

#[cfg(test)]
mod tests {
    use crate::{Schema, SchemaType, TypeSet};

    #[test]
    fn an_object_macro_writes_properties_and_required_together() {
        let built = object! {
            required "id": Schema::string(),
            "nickname": Schema::string(),
            required "email": Schema::string(),
        }
        .build()
        .unwrap();

        let by_hand = Schema::object()
            .required_property("id", Schema::string())
            .property("nickname", Schema::string())
            .required_property("email", Schema::string())
            .build()
            .unwrap();

        assert_eq!(built, by_hand);
        assert_eq!(
            built.as_object().unwrap().required.as_ref().unwrap(),
            &["id", "email"],
        );
    }

    #[test]
    fn an_empty_object_macro_is_a_bare_object_schema() {
        assert_eq!(
            object! {}.build().unwrap(),
            Schema::object().build().unwrap()
        );
    }

    #[test]
    fn a_trailing_comma_is_optional() {
        let with = object! { "a": Schema::string(), }.build().unwrap();
        let without = object! { "a": Schema::string() }.build().unwrap();
        assert_eq!(with, without);
    }

    #[test]
    fn the_result_is_a_builder_the_rest_of_the_vocabulary_still_works_on() {
        let schema = object! { required "id": Schema::string() }
            .title("User")
            .min_properties(1)
            .closed()
            .build()
            .unwrap();

        let object = schema.as_object().unwrap();
        assert_eq!(object.title.as_deref(), Some("User"));
        assert_eq!(object.min_properties, Some(1));
        assert_eq!(object.types, Some(TypeSet::One(SchemaType::Object)));
    }

    #[test]
    fn objects_nest_without_an_inner_build() {
        let schema = object! {
            required "user": object! {
                required "id": Schema::string(),
                "profile": object! { "bio": Schema::string() },
            },
        }
        .build()
        .unwrap();

        let user = schema.property("user").unwrap();
        assert!(user.requires("id"));
        assert!(user.property("profile").unwrap().property("bio").is_some());
    }

    #[test]
    fn properties_keep_the_order_they_were_written_in() {
        let schema = object! {
            "z": Schema::string(),
            required "a": Schema::string(),
            "m": Schema::string(),
        }
        .build()
        .unwrap();

        let names: Vec<_> = schema
            .as_object()
            .unwrap()
            .properties
            .as_ref()
            .unwrap()
            .keys()
            .collect();
        assert_eq!(names, ["z", "a", "m"]);
    }

    #[test]
    fn a_property_name_may_be_computed_when_parenthesised() {
        let tag = "x-request-id";
        let schema = object! {
            required (tag): Schema::string(),
            (format!("x-{}", "trace")): Schema::string(),
            "literal": Schema::string(),
        }
        .build()
        .unwrap();

        assert!(schema.requires("x-request-id"));
        assert!(schema.property("x-trace").is_some());
        assert!(schema.property("literal").is_some());
    }

    #[test]
    fn a_definition_name_may_be_computed_too() {
        let name = "Money";
        let schema = defs! {
            Schema::builder();
            (name): Schema::object(),
            "Sku": Schema::string(),
        }
        .build()
        .unwrap();

        assert!(schema.def("Money").is_some());
        assert!(schema.def("Sku").is_some());
    }

    #[test]
    fn defs_adds_each_definition_and_leaves_the_builder_alone() {
        let built = defs! {
            Schema::object().title("Root");

            "A": Schema::string(),
            "B": Schema::integer(),
        }
        .property("a", Schema::reference("#/$defs/A"))
        .build()
        .unwrap();

        assert_eq!(built.as_object().unwrap().title.as_deref(), Some("Root"));
        assert!(built.def("A").is_some());
        assert!(built.def("B").is_some());
        assert!(built.property("a").is_some());
    }

    #[test]
    fn defs_matches_the_calls_it_stands_for() {
        let built = defs! {
            Schema::builder();
            "A": Schema::string(),
            "B": Schema::integer(),
        }
        .build()
        .unwrap();

        let by_hand = Schema::builder()
            .def("A", Schema::string())
            .def("B", Schema::integer())
            .build()
            .unwrap();

        assert_eq!(built, by_hand);
    }

    #[test]
    fn a_tagged_union_pins_the_tag_in_every_branch() {
        let schema = one_of_tagged! {
            "kind";
            "circle" => object! { required "radius": Schema::number() },
            "rect" => object! {
                required "width": Schema::number(),
                required "height": Schema::number(),
            },
        }
        .build()
        .unwrap();

        let branches = schema.as_object().unwrap().one_of.as_ref().unwrap();
        assert_eq!(branches.len(), 2);

        for (branch, tag) in branches.iter().zip(["circle", "rect"]) {
            assert!(branch.requires("kind"), "the tag must be required");
            let pinned = branch.property("kind").unwrap();
            assert_eq!(
                pinned.as_object().unwrap().constant,
                Some(serde_json::json!(tag)),
            );
        }
    }

    #[test]
    fn a_tagged_union_is_a_builder_too() {
        let schema = one_of_tagged! {
            "kind";
            "a" => object! { "x": Schema::string() },
        }
        .title("Shape")
        .build()
        .unwrap();

        assert_eq!(schema.as_object().unwrap().title.as_deref(), Some("Shape"));
    }

    #[test]
    fn everything_the_macros_write_passes_the_meta_schema() {
        let schema = defs! {
            Schema::object().dialect().id("https://example.com/s");

            "Shape": one_of_tagged! {
                "kind";
                "circle" => object! { required "radius": Schema::number() },
            },
        }
        .property("shape", Schema::reference("#/$defs/Shape"))
        .build();

        assert!(schema.is_ok(), "{:?}", schema.unwrap_err());
    }
}

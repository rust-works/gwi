//! Shared, fail-closed production boundary for MCP source tripwires.

#![cfg(test)]

use syn::spanned::Spanned;
use syn::{Attribute, Item};

fn test_only(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("cfg")
            && attribute
                .parse_args::<syn::Path>()
                .is_ok_and(|path| path.is_ident("test"))
    })
}

/// Preserve the original text for grep-style checks, but only truncate at a
/// structurally verified trailing test module. Other cfg forms stay scanned.
pub(super) fn production(source: &str) -> Result<&str, String> {
    let file = syn::parse_file(source).map_err(|error| format!("cannot parse source: {error}"))?;
    if test_only(&file.attrs) {
        return Ok("");
    }
    for (index, item) in file.items.iter().enumerate() {
        let attributes = match item {
            Item::Const(item) => &item.attrs,
            Item::Enum(item) => &item.attrs,
            Item::ExternCrate(item) => &item.attrs,
            Item::Fn(item) => &item.attrs,
            Item::ForeignMod(item) => &item.attrs,
            Item::Impl(item) => &item.attrs,
            Item::Macro(item) => &item.attrs,
            Item::Mod(item) => &item.attrs,
            Item::Static(item) => &item.attrs,
            Item::Struct(item) => &item.attrs,
            Item::Trait(item) => &item.attrs,
            Item::TraitAlias(item) => &item.attrs,
            Item::Type(item) => &item.attrs,
            Item::Union(item) => &item.attrs,
            Item::Use(item) => &item.attrs,
            _ => return Err("unsupported source item; cannot verify test boundary".to_owned()),
        };
        if !test_only(attributes) {
            continue;
        }
        if !matches!(item, Item::Mod(module) if module.content.is_some())
            || index + 1 != file.items.len()
        {
            return Err(format!(
                "line {}: #[cfg(test)] must gate a final inline test module; move test helpers into that module and production items before it",
                item.span().start().line
            ));
        }
        return Ok(&source[..item.span().byte_range().start]);
    }
    Ok(source)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Test assertions unwrap the expected successful parse.
mod tests {
    use super::*;

    #[test]
    fn trailing_module_excludes_only_fixture_code() {
        let prefix = "fn production() {}\n";
        let source = format!(
            "{prefix}#[cfg( /* comment */ test )]\n#[allow(dead_code)]\npub(crate) mod tests {{
                fn fixture() {{
                    if true {{ std::fs::write(path, bytes); api.upload(path); }}
                    let normal = \"}} \\\" #[cfg(test)] {{\";
                    let raw = r###\"}} #[cfg(test)] {{\"###;
                    let character = '}}';
                    // }} #[cfg(test)]
                    /* {{ /* nested }} */ #[cfg(test)] */
                }}
            }}\n// trailing comment"
        );
        assert_eq!(production(&source).unwrap(), prefix);
    }

    #[test]
    fn boundary_text_in_literals_and_comments_does_not_truncate() {
        let source = r####"
            // #[cfg(test)] mod tests {}
            /* #[cfg(test)] /* nested comment */ mod tests {} */
            const TEXT: &str = "#[cfg(test)] }";
            const RAW: &str = r###"#[cfg(test)] {"###;
            fn production() { std::fs::write(path, bytes); }
        "####;
        assert_eq!(production(source).unwrap(), source);
    }

    #[test]
    fn production_after_test_boundary_is_rejected() {
        for item in [
            "fn production() { std::fs::write(path, bytes); }",
            "struct NewParams { pub source_path: Option<String> }",
            "fn production() { api.upload(path); }",
        ] {
            for boundary in [
                "#[cfg(test)] mod tests { fn fixture() { if true { let x = \"}\"; } } }",
                "#[cfg(test)] fn helper() {}",
            ] {
                let source = format!("{boundary}\n{item}");
                let error = production(&source).unwrap_err();
                assert!(error.contains("final inline test module"), "{error}");
            }
        }
    }

    #[test]
    fn non_module_and_multiple_boundaries_are_rejected() {
        for source in [
            "#[cfg(test)] fn helper() {}",
            "#[cfg(test)] mod tests;",
            "#[cfg(test)] mod first {} #[cfg(test)] mod second {}",
            "fn before() {} #[cfg(test)] struct Helper;",
        ] {
            assert!(production(source).is_err(), "{source}");
        }
    }

    #[test]
    fn whole_test_only_files_are_excluded() {
        assert_eq!(
            production("#![cfg(test)] fn fixture() { api.upload(path); }").unwrap(),
            ""
        );
    }

    #[test]
    fn other_cfg_forms_remain_scanned() {
        let source = "#[cfg(not(test))] fn production() {} #[cfg(all(test, unix))] mod tests {}";
        assert_eq!(production(source).unwrap(), source);
    }

    #[test]
    fn production_item_kinds_remain_scanned() {
        let source = r#"
            const VALUE: u8 = 1;
            enum Kind { One }
            extern crate example;
            fn production() {}
            extern "C" { fn foreign(); }
            impl Kind {}
            macro_rules! example { () => {} }
            mod nested {}
            static STATIC: u8 = 1;
            struct Params;
            trait Trait {}
            trait Alias = Trait;
            type Value = u8;
            union Union { value: u8 }
            use std::path::Path;
        "#;
        assert_eq!(production(source).unwrap(), source);
    }

    #[test]
    fn malformed_source_fails_closed() {
        for source in ["#[cfg(test)] mod tests {", "fn production( {}"] {
            assert!(production(source)
                .unwrap_err()
                .contains("cannot parse source"));
        }
    }

    #[test]
    fn unicode_before_boundary_preserves_byte_offsets() {
        let prefix = "const TEXT: &str = \"日🦀\";\n";
        let source = format!("{prefix}#[cfg(test)] mod tests {{}}");
        assert_eq!(production(&source).unwrap(), prefix);
    }
}

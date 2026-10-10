//! Source-level guard for paired rustdoc and CLI/MCP presentation descriptions.

#![allow(clippy::unwrap_used, clippy::expect_used)] // Test setup and assertions.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Attribute, Expr, Item, Lit, Meta, Token};

type Macros = BTreeMap<String, String>;

// Only the two existing zero-argument, single-string authoring macros are supported.
// Parsing their bodies instead of copying text here keeps semantic changes visible.
fn account_macros(items: &[Item]) -> Macros {
    items
        .iter()
        .filter_map(|item| {
            let Item::Macro(item) = item else { return None };
            let name = item.ident.as_ref()?.to_string();
            if !matches!(name.as_str(), "account_param_doc" | "account_param_plain") {
                return None;
            }
            let tokens: Vec<_> = item.mac.tokens.clone().into_iter().collect();
            assert_eq!(tokens.len(), 5, "unsupported {name} definition");
            assert!(
                matches!(&tokens[0], proc_macro2::TokenTree::Group(g) if g.stream().is_empty())
            );
            assert!(matches!(&tokens[1], proc_macro2::TokenTree::Punct(p) if p.as_char() == '='));
            assert!(matches!(&tokens[2], proc_macro2::TokenTree::Punct(p) if p.as_char() == '>'));
            assert!(matches!(&tokens[4], proc_macro2::TokenTree::Punct(p) if p.as_char() == ';'));
            let proc_macro2::TokenTree::Group(body) = &tokens[3] else {
                panic!("unsupported {name} body")
            };
            let literal: syn::LitStr =
                syn::parse2(body.stream()).expect("single string macro body");
            Some((name, literal.value()))
        })
        .collect()
}

fn text(expr: &Expr, macros: &Macros) -> Result<String, String> {
    match expr {
        Expr::Lit(lit) => match &lit.lit {
            Lit::Str(value) => Ok(value.value()),
            _ => Err("description must be a string".into()),
        },
        Expr::Macro(expr) if expr.mac.tokens.is_empty() => {
            let name = expr.mac.path.get_ident().map(ToString::to_string);
            name.and_then(|name| macros.get(&name).cloned())
                .ok_or_else(|| "unsupported description macro (add an explicit resolver)".into())
        }
        _ => Err("unsupported description expression (add an explicit resolver)".into()),
    }
}

// This is deliberately a small authoring convention, not a Markdown renderer.
fn plain(doc: &str) -> String {
    assert!(
        !doc.contains('\0'),
        "description contains reserved NUL delimiter"
    );
    static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`\n]+)`").unwrap());
    static LINKS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[([^\]\n]+)\]\([^\)\n]+\)").unwrap());
    static REFERENCES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[(\x00[0-9]+\x00)\]").unwrap());
    static EMPHASIS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\*{1,2}([^*\n]+)\*{1,2}").unwrap());
    let mut literals = Vec::new();
    let protected = CODE.replace_all(doc, |span: &regex::Captures<'_>| {
        let token = format!("\0{}\0", literals.len());
        literals.push(span[1].to_owned());
        token
    });
    let prose = LINKS.replace_all(&protected, "$1");
    let prose = REFERENCES.replace_all(&prose, "$1");
    let mut result = EMPHASIS.replace_all(&prose, "$1").into_owned();
    for (index, literal) in literals.iter().enumerate() {
        result = result.replace(&format!("\0{index}\0"), literal);
    }
    result
}

fn whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn check_attrs(attrs: &[Attribute], name: &str, macros: &Macros) -> Result<usize, String> {
    let mut descriptions = Vec::new();
    for attr in attrs {
        if !["arg", "command", "value", "schemars"]
            .iter()
            .any(|path| attr.path().is_ident(path))
        {
            continue;
        }
        let entries = attr
            .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            .map_err(|error| {
                format!(
                    "{name}:{}: cannot parse presentation attribute: {error}",
                    attr.span().start().line
                )
            })?;
        for entry in entries {
            let Meta::NameValue(value) = entry else {
                continue;
            };
            let key = value
                .path
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();
            if !matches!(
                key.as_str(),
                "help" | "long_help" | "about" | "long_about" | "description"
            ) {
                continue;
            }
            // Explicit None disables clap's derived long description, not a prose pair.
            if matches!(&value.value, Expr::Path(p) if p.path.is_ident("None")) {
                continue;
            }
            let display = text(&value.value, macros)
                .map_err(|error| format!("{name}:{} {key}: {error}", value.span().start().line))?;
            descriptions.push((key, display, value.span().start().line));
        }
    }
    if descriptions.is_empty() {
        return Ok(0);
    }
    let mut lines = Vec::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("doc")) {
        let Meta::NameValue(value) = &attr.meta else {
            continue;
        };
        let line = text(&value.value, macros)
            .map_err(|error| format!("{name}:{} rustdoc: {error}", attr.span().start().line))?;
        lines.push(line.trim().to_owned());
    }
    // Explicit independently authored text without rustdoc has no pair to compare.
    if lines.is_empty() {
        return Ok(0);
    }
    let doc = lines.join("\n");
    for (key, display, line) in &descriptions {
        let short = matches!(key.as_str(), "help" | "about");
        let expected = if short {
            doc.split("\n\n").next().unwrap()
        } else {
            &doc
        };
        let expected = whitespace(expected);
        let mut actual = whitespace(display);
        // These two fields quote literal Markdown output syntax in terminal help.
        // Replace only the exact examples, preserving every other word and value.
        if matches!(
            name,
            "src/cli/gmail/read.rs::MessageOutputArgs::fold_quotes"
                | "src/cli/gmail/render.rs::RenderCommand::fold_quotes"
        ) {
            for (doc, help) in [
                ("`>`", "'>'"),
                (
                    "`*(N quoted lines omitted)*`",
                    "'*(N quoted lines omitted)*'",
                ),
            ] {
                actual = actual.replace(help, doc);
            }
        }
        if matches!(
            name,
            "src/cli/format.rs::OutputFormat::Yamls"
                | "src/cli/gmail/read.rs::ReadOutputFormat::Yamls"
        ) {
            actual = actual.replace("'---'", "`---`");
        }
        // Older display overrides retain brackets around shorthand Rustdoc links.
        // Accept only labels actually linked in this paired doc, not arbitrary
        // bracketed values (which may themselves be literal output syntax).
        static DOC_LINKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[`([^`]+)`\]").unwrap());
        for link in DOC_LINKS.captures_iter(&expected) {
            actual = actual.replace(&format!("[{}]", &link[1]), &link[1]);
        }
        let expected = plain(&expected);
        let actual = plain(&actual);
        let (expected, actual) = if short {
            (
                expected.strip_suffix('.').unwrap_or(&expected),
                actual.strip_suffix('.').unwrap_or(&actual),
            )
        } else {
            (expected.as_str(), actual.as_str())
        };
        if expected != actual {
            return Err(format!("{name}:{line} {key} differs from rustdoc\n  rustdoc: {expected}\n  display: {actual}"));
        }
    }
    Ok(descriptions.len())
}

fn check_items(items: &[Item], prefix: &str, macros: &Macros) -> Result<usize, String> {
    let mut count = 0;
    for item in items {
        match item {
            Item::Struct(item) => {
                let name = format!("{prefix}::{}", item.ident);
                count += check_attrs(&item.attrs, &name, macros)?;
                count += check_fields(&item.fields, &name, macros)?;
            }
            Item::Enum(item) => {
                let name = format!("{prefix}::{}", item.ident);
                count += check_attrs(&item.attrs, &name, macros)?;
                for variant in &item.variants {
                    let name = format!("{name}::{}", variant.ident);
                    count += check_attrs(&variant.attrs, &name, macros)?;
                    count += check_fields(&variant.fields, &name, macros)?;
                }
            }
            Item::Mod(item) => {
                if item.attrs.iter().any(|attr| {
                    attr.path().is_ident("cfg")
                        && attr
                            .parse_args::<syn::Path>()
                            .is_ok_and(|path| path.is_ident("test"))
                }) {
                    continue;
                }
                if let Some((_, items)) = &item.content {
                    count += check_items(items, &format!("{prefix}::{}", item.ident), macros)?;
                }
            }
            _ => {} // Functions/tool attributes are independently authored.
        }
    }
    Ok(count)
}

fn check_fields(fields: &syn::Fields, prefix: &str, macros: &Macros) -> Result<usize, String> {
    let mut count = 0;
    for (index, field) in fields.iter().enumerate() {
        let name = field
            .ident
            .as_ref()
            .map_or_else(|| index.to_string(), ToString::to_string);
        count += check_attrs(&field.attrs, &format!("{prefix}::{name}"), macros)?;
    }
    Ok(count)
}

// Resolve the actual parsed imports, so strings/comments cannot enable a resolver.
fn imports_drive_accounts(items: &[Item]) -> bool {
    fn imported(tree: &syn::UseTree, prefix: &str, names: &mut Vec<String>) {
        match tree {
            syn::UseTree::Path(path) => {
                imported(&path.tree, &format!("{prefix}{}::", path.ident), names);
            }
            syn::UseTree::Group(group) => {
                for tree in &group.items {
                    imported(tree, prefix, names);
                }
            }
            syn::UseTree::Name(name) => names.push(format!("{prefix}{}", name.ident)),
            _ => {} // Aliases/globs are unsupported and fail when used as descriptions.
        }
    }
    let mut names = Vec::new();
    for item in items {
        if let Item::Use(item) = item {
            imported(&item.tree, "", &mut names);
        }
    }
    ["account_param_doc", "account_param_plain"]
        .iter()
        .all(|name| names.contains(&format!("crate::mcp::drive_tools::{name}")))
}

// Stable labels also key the source-specific mappings on Windows.
fn source_label(relative: &Path) -> String {
    relative
        .components()
        .map(|part| part.as_os_str().to_str().unwrap())
        .collect::<Vec<_>>()
        .join("/")
}

fn rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

#[test]
fn explicit_descriptions_match_rustdoc() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let drive =
        syn::parse_file(&std::fs::read_to_string(root.join("src/mcp/drive_tools.rs")).unwrap())
            .unwrap();
    let drive_macros = account_macros(&drive.items);
    let mut files = vec![root.join("src/cli.rs")];
    rust_files(&root.join("src/cli"), &mut files);
    rust_files(&root.join("src/mcp"), &mut files);
    files.sort();
    let mut errors = Vec::new();
    let mut count = 0;
    for path in files {
        let source = std::fs::read_to_string(&path).unwrap();
        let file = syn::parse_file(&source).unwrap();
        let mut macros = account_macros(&file.items);
        // Drive's sibling modules explicitly import these shared macros. Gmail has
        // local definitions; never replace them with the Drive wording.
        if imports_drive_accounts(&file.items) {
            macros.extend(drive_macros.clone());
        }
        match check_items(
            &file.items,
            &source_label(path.strip_prefix(root).unwrap()),
            &macros,
        ) {
            Ok(n) => count += n,
            Err(error) => errors.push(error),
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n\n"));
    assert!(
        count > 200,
        "unexpectedly small paired-description inventory: {count}"
    );
    println!("Checked {count} explicit description pairs");
}

fn check_fixture(source: &str) -> Result<usize, String> {
    let file = syn::parse_file(source).unwrap();
    check_items(&file.items, "fixture.rs", &account_macros(&file.items))
}

#[test]
fn rustdoc_only_semantic_edits_fail_for_both_surfaces() {
    for attribute in ["arg(help", "schemars(description"] {
        let source = format!(
            r#"struct Params {{
            /// Omit to use the `configured default`.
            #[{attribute} = "Omit to use the configured default.")]
            account: String,
        }}"#
        );
        assert_eq!(check_fixture(&source).unwrap(), 1);
        let error =
            check_fixture(&source.replace("`configured default`", "`sole account`")).unwrap_err();
        assert!(error.contains("fixture.rs::Params::account:3"), "{error}");
        assert!(error.contains("differs from rustdoc"), "{error}");
    }
}

#[test]
fn formatting_short_and_full_paragraphs_pass_without_hiding_long_drift() {
    let source = r#"
        /// Read `file_id` from [Drive](https://example.com).
        ///
        /// Omit to use the *default*.
        #[command(about = "Read file_id from Drive", long_about = "Read file_id from Drive.\n\nOmit to use the default.")]
        struct Read;
    "#;
    assert_eq!(check_fixture(source).unwrap(), 2);
    assert!(check_fixture(&source.replace("*default*", "*other account*")).is_err());
}

#[test]
fn unrelated_comments_and_explicit_tool_markdown_are_not_pairs() {
    assert_eq!(
        check_fixture(
            r#"
        //! unrelated `module` documentation
        /// independent tool prose
        #[tool(description = "Keep `intentional Markdown`.")]
        fn tool() {}
        /// No override, so nothing to drift.
        struct Unpaired { field: String }
    "#
        )
        .unwrap(),
        0
    );
}

#[test]
fn unsupported_description_expressions_fail_closed() {
    for expression in ["other_macro!()", "DESCRIPTION", "concat!(\"one\", \"two\")"] {
        let source = format!("struct Params {{ /// Description.\n #[schemars(description = {expression})] value: String }}");
        assert!(check_fixture(&source)
            .unwrap_err()
            .contains("unsupported description"));
    }
}

#[test]
fn shared_account_macro_edits_are_checked_at_each_parameter() {
    let source = r#"
        macro_rules! account_param_doc { () => { "Omit for `default`." }; }
        macro_rules! account_param_plain { () => { "Omit for default." }; }
        struct Params {
            #[doc = account_param_doc!()]
            #[schemars(description = account_param_plain!())]
            account: String,
        }
    "#;
    assert_eq!(check_fixture(source).unwrap(), 1);
    assert!(check_fixture(&source.replace("`default`", "`legacy`"))
        .unwrap_err()
        .contains("Params::account"));
}

#[test]
fn literal_yaml_and_markdown_examples_pass_but_semantic_edits_fail() {
    for (path, source) in [
        (
            "src/cli/format.rs",
            r#"enum OutputFormat {
            /// YAML stream (`---`-separated multi-document).
            #[value(help = "YAML stream ('---'-separated multi-document)")]
            Yamls,
        }"#,
        ),
        (
            "src/cli/gmail/read.rs",
            r#"struct MessageOutputArgs {
            /// Collapses `>` history into a `*(N quoted lines omitted)*` marker. Off by default.
            #[arg(help = "Collapses '>' history into a '*(N quoted lines omitted)*' marker. Off by default")]
            fold_quotes: bool,
        }"#,
        ),
    ] {
        let check = |source: &str| {
            let file = syn::parse_file(source).unwrap();
            check_items(&file.items, path, &Macros::new())
        };
        assert_eq!(check(source).unwrap(), 1);
        let changed = source
            .replacen("multi-document", "single-document", 1)
            .replacen("Off by default.", "On by default.", 1);
        assert!(check(&changed).is_err());
        let changed = source.replacen("`---`", "`--`", 1).replacen(
            "N quoted lines omitted",
            "N lines removed",
            1,
        );
        assert!(check(&changed).is_err());
    }
}

#[test]
fn inline_code_contents_and_readable_links_are_preserved() {
    assert_eq!(
        plain("See [`Params`] and *`account`* or **default**."),
        "See Params and account or default."
    );
    assert_eq!(
        plain("Literal `*required*` and `[title](url)`"),
        "Literal *required* and [title](url)"
    );
    assert!(check_fixture(r#"struct Params {
        /// Example: `[["name", "score"], ["Ada", "42"]]`. Omit for `false`.
        #[schemars(description = "Example: [[\"name\", \"score\"], [\"Ada\", \"42\"]]. Omit for false.")]
        values: String,
    }"#).is_ok());
}

#[test]
fn many_code_spans_preserve_each_literal_without_prefix_collisions() {
    let doc = (0..15)
        .map(|index| format!("`literal{index}`"))
        .collect::<Vec<_>>()
        .join(" ");
    let expected = (0..15)
        .map(|index| format!("literal{index}"))
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(plain(&doc), expected);
}

#[test]
fn parsed_imports_cannot_be_enabled_by_comments_or_strings() {
    let fake = syn::parse_file(
        r#"
        // use crate::mcp::drive_tools::{account_param_doc, account_param_plain};
        const TEXT: &str = "use crate::mcp::drive_tools::{account_param_doc, account_param_plain};";
    "#,
    )
    .unwrap();
    assert!(!imports_drive_accounts(&fake.items));
    let real =
        syn::parse_file("use crate::mcp::drive_tools::{account_param_doc, account_param_plain};")
            .unwrap();
    assert!(imports_drive_accounts(&real.items));
    assert_eq!(
        check_fixture(
            r#"
        #[cfg(test)] mod tests {
            struct Fixture {
                /// Unrelated fixture text.
                #[arg(help = "Intentionally different")]
                field: String,
            }
        }
    "#
        )
        .unwrap(),
        0
    );
}

#[test]
fn shorthand_link_labels_do_not_strip_unrelated_bracketed_literals() {
    assert_eq!(
        check_fixture(
            r#"struct Params {
        /// See [`module::resolve`] and literal `[Type]`.
        #[arg(help = "See [module::resolve] and literal [Type]")]
        field: String,
    }"#
        )
        .unwrap(),
        1
    );
}

#[test]
fn native_paths_use_portable_labels_for_source_specific_mappings() {
    let relative: PathBuf = ["src", "cli", "gmail", "read.rs"].iter().collect();
    assert_eq!(source_label(&relative), "src/cli/gmail/read.rs");
    let file = syn::parse_file(
        r#"enum ReadOutputFormat {
        /// YAML stream (`---`-separated multi-document).
        #[value(help = "YAML stream ('---'-separated multi-document)")]
        Yamls,
    }"#,
    )
    .unwrap();
    assert_eq!(
        check_items(&file.items, &source_label(&relative), &Macros::new()).unwrap(),
        1
    );
}

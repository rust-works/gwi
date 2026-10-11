//! Source tripwires for caller-chosen MCP paths; not alias analysis.
//!
//! Parameter ownership comes from Rust syntax: public named `Option<String>`
//! fields named `output_file` or ending in `_path` require review. Macros are not expanded.
//!
//! Covers fs reads/writes/copy/rename/removal, File open/create/create_new, and
//! OpenOptions with std/tokio or imported fs/File spellings. Direct imports of
//! guarded functions (including renamed, nested/grouped and glob imports) are
//! rejected at the import, as are renamed std/tokio fs modules and File imports
//! (flat, grouped/nested, and self-as forms). Unrenamed fs/File imports are allowed.
//! Arbitrary type aliases, re-exports, indirect alias chains, macros, and
//! comments/strings in filesystem checks are not resolved. Only tool files are scanned; policy helpers
//! and trailing tests remain outside the production scan.

#![cfg(test)]

use std::collections::BTreeSet;
use std::path::Path;

use regex::Regex;
use syn::visit::Visit;
use syn::{GenericArgument, PathArguments, Type, Visibility};

use super::source_guard::production;

// Add an entry only after routing the parameter through PathPolicy. Keep the struct
// name: allowing `output_file` throughout a file would silently admit a new tool.
const POLICY_CHECKED_PARAMS: &[(&str, &str, &str)] = &[
    // require_bounded_content_input -> PathPolicy::check_read
    ("drive_write_tools.rs", "DriveDocsAppendParams", "text_path"),
    (
        "drive_write_tools.rs",
        "DriveSheetsWriteParams",
        "values_path",
    ),
    // check_output_file / write_*_to_file_yaml -> PathPolicy::check_write
    ("drive_tools.rs", "DriveFileReadParams", "output_file"),
    ("drive_docs_tools.rs", "DriveDocsReadParams", "output_file"),
    (
        "drive_sheets_tools.rs",
        "DriveSheetsReadParams",
        "output_file",
    ),
    ("gmail_tools.rs", "GmailMessageReadParams", "output_file"),
    ("gmail_tools.rs", "GmailDraftShowParams", "output_file"),
];

fn optional_string(ty: &Type) -> bool {
    let Type::Path(ty) = ty else {
        return false;
    };
    if ty.qself.is_some() || ty.path.leading_colon.is_some() || ty.path.segments.len() != 1 {
        return false;
    }
    let segment = &ty.path.segments[0];
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return false;
    };
    segment.ident == "Option"
        && arguments.args.len() == 1
        && matches!(&arguments.args[0], GenericArgument::Type(Type::Path(inner))
            if inner.qself.is_none() && inner.path.is_ident("String"))
}

#[derive(Default)]
struct PathParams(Vec<(String, String)>);

impl<'ast> Visit<'ast> for PathParams {
    fn visit_item_struct(&mut self, item: &'ast syn::ItemStruct) {
        for field in &item.fields {
            let Some(name) = &field.ident else {
                continue;
            };
            let name = name.to_string();
            if matches!(field.vis, Visibility::Public(_))
                && (name == "output_file" || name.ends_with("_path"))
                && optional_string(&field.ty)
            {
                self.0.push((item.ident.to_string(), name));
            }
        }
        syn::visit::visit_item_struct(self, item);
    }
}

fn path_params(source: &str) -> Vec<(String, String)> {
    let source = production(source).unwrap();
    let file = syn::parse_file(source).unwrap();
    let mut params = PathParams::default();
    params.visit_file(&file);
    params.0
}

fn stale_params(found: &BTreeSet<(String, String, String)>) -> Vec<String> {
    POLICY_CHECKED_PARAMS
        .iter()
        .filter(|&&(file, owner, field)| {
            !found.contains(&(file.to_owned(), owner.to_owned(), field.to_owned()))
        })
        .map(|&(file, owner, field)| format!("{file}: {owner}.{field}"))
        .collect()
}

fn unchecked_params(file: &str, source: &str) -> Vec<String> {
    path_params(source)
        .into_iter()
        .filter(|(owner, field)| {
            !POLICY_CHECKED_PARAMS.contains(&(file, owner.as_str(), field.as_str()))
        })
        .map(|(owner, field)| {
            format!("{file}: {owner}.{field} must go through PathPolicy before being allowlisted")
        })
        .collect()
}

// Expand conventional use trees, preserving whether each leaf is renamed. Check
// original paths at the import rather than resolving subsequent local-name calls.
fn imported_paths(tree: &str, prefix: &str, paths: &mut Vec<(String, bool)>) {
    let mut depth = 0;
    let mut start = 0;
    for (index, character) in tree.char_indices() {
        match character {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                imported_paths(&tree[start..index], prefix, paths);
                start = index + 1;
            }
            _ => {}
        }
    }
    if start > 0 {
        imported_paths(&tree[start..], prefix, paths);
        return;
    }
    let tree = tree.trim();
    if tree.is_empty() {
        return;
    }
    if let Some((parent, children)) = tree.split_once('{') {
        let parent: String = parent.chars().filter(|c| !c.is_whitespace()).collect();
        imported_paths(
            children.trim_end().strip_suffix('}').unwrap_or(children),
            &format!("{prefix}{parent}"),
            paths,
        );
    } else {
        let mut parts = tree.split_whitespace();
        let original = parts
            .by_ref()
            .take_while(|part| *part != "as")
            .collect::<String>();
        let renamed = parts.next().is_some();
        let path = if original == "self" {
            prefix.trim_end_matches("::").to_owned()
        } else {
            format!("{prefix}{original}")
        };
        paths.push((path, renamed));
    }
}

fn direct_filesystem_calls(source: &str) -> Vec<String> {
    // Retain read* coverage (including read_dir), plus operations that can replace
    // or remove caller-chosen files. OpenOptions also catches aliased builders.
    let calls = Regex::new(
        r"\b(?:(?:(?:std|tokio)\s*::\s*)?fs\s*::\s*(?:write|read\w*|copy|rename|remove_file|remove_dir|remove_dir_all)\s*\(|File\s*::\s*(?:open|create|create_new)\s*\(|OpenOptions\b)",
    )
    .unwrap();
    let source = production(source).unwrap();
    let mut found: Vec<_> = calls
        .find_iter(source)
        .map(|call| call.as_str().to_owned())
        .collect();
    let imports = Regex::new(r"\buse\s+([^;]+);").unwrap();
    let forbidden = Regex::new(
        r"^(?:std|tokio)::fs::(?:write|read\w*|copy|rename|remove_file|remove_dir|remove_dir_all|\*)$",
    )
    .unwrap();
    let renamed_filesystem = Regex::new(r"^(?:std|tokio)::fs(?:::File)?$").unwrap();
    for import in imports.captures_iter(source) {
        let mut paths = Vec::new();
        imported_paths(&import[1], "", &mut paths);
        if paths.iter().any(|(path, renamed)| {
            let path = path.trim_start_matches("::");
            forbidden.is_match(path) || (*renamed && renamed_filesystem.is_match(path))
        }) {
            found.push(import[0].to_owned());
        }
    }
    found
}

fn filesystem_policy_errors(file: &str, source: &str) -> Vec<String> {
    direct_filesystem_calls(source)
        .into_iter()
        .map(|call| {
            format!("{file}: direct filesystem access {call:?}; use the PathPolicy helpers in content_input.rs or output_file.rs")
        })
        .collect()
}

#[test]
fn mcp_tool_paths_require_policy_review() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/mcp");
    let mut found = BTreeSet::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let file = path.file_name().unwrap().to_str().unwrap();
        if !file.ends_with("_tools.rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        production(&source).unwrap_or_else(|error| panic!("{file}: {error}"));
        let unchecked = unchecked_params(file, &source);
        assert!(unchecked.is_empty(), "{}", unchecked.join("\n"));
        for (owner, field) in path_params(&source) {
            found.insert((file.to_owned(), owner, field));
        }
        let errors = filesystem_policy_errors(file, &source);
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }
    let stale = stale_params(&found);
    assert!(
        stale.is_empty(),
        "update stale PathPolicy allowlist entries: {}",
        stale.join("\n")
    );
}

#[test]
fn new_path_parameters_require_policy_review() {
    for (owner, field) in [
        ("NewParams", "source_path"),
        ("NewParams", "output_file"),
        ("GmailMessageReadParams", "attachment_path"),
    ] {
        let source = format!("pub struct {owner} {{ pub {field} : Option < String >, }}");
        let errors = unchecked_params("gmail_tools.rs", &source);
        assert_eq!(errors.len(), 1, "{source}");
        assert!(errors[0].contains(&format!("{owner}.{field}")));
        assert!(errors[0].contains("PathPolicy"));
    }
}

#[test]
fn known_paths_and_non_path_parameters_are_accepted() {
    let source = "pub struct GmailMessageReadParams {
        pub output_file: Option<String>,
        pub query: Option<String>,
    }";
    assert!(unchecked_params("gmail_tools.rs", source).is_empty());
    assert_eq!(path_params(source).len(), 1);
    assert_eq!(unchecked_params("new_tools.rs", source).len(), 1);
    let extended = format!("{source}\nstruct NewParams {{ pub output_file: Option<String> }}");
    assert_eq!(unchecked_params("gmail_tools.rs", &extended).len(), 1);
}

#[test]
fn comments_and_strings_cannot_spoof_allowlisted_ownership() {
    for decoy in [
        "// struct GmailMessageReadParams {",
        "/* struct GmailMessageReadParams { */",
        "/* outer /* struct GmailMessageReadParams { */ nested */",
        "#[doc = \"struct GmailMessageReadParams {\"]",
        "#[doc = r###\"struct GmailMessageReadParams {\"###]",
    ] {
        let source = format!(
            "struct GmailMessageReadParams {{ pub output_file: Option<String> }}
             struct NewParams {{ {decoy}
 pub output_file: Option<String> }}"
        );
        assert_eq!(
            path_params(&source),
            [
                (
                    "GmailMessageReadParams".to_owned(),
                    "output_file".to_owned()
                ),
                ("NewParams".to_owned(), "output_file".to_owned()),
            ],
            "{source}"
        );
        assert_eq!(
            unchecked_params("gmail_tools.rs", &source),
            ["gmail_tools.rs: NewParams.output_file must go through PathPolicy before being allowlisted"],
            "{source}"
        );
    }
}

#[test]
fn field_shaped_comments_and_literals_do_not_create_parameters() {
    let source = r####"
        // struct Fake { pub comment_path: Option<String> }
        /* struct Fake { /* nested */ pub block_path: Option<String> } */
        const TEXT: &str = "struct Fake { pub string_path: Option<String> }";
        const RAW: &str = r###"struct Fake { pub raw_path: Option<String> }"###;
        struct Real {
            // pub comment_path: Option<String>,
            /* pub block_path: Option<String>, */
            #[doc = "pub string_path: Option<String>"]
            #[doc = r###"pub raw_path: Option<String>"###]
            pub source_path: Option<String>,
        }
    "####;
    assert_eq!(
        path_params(source),
        [("Real".to_owned(), "source_path".to_owned())]
    );
}

#[test]
fn real_structs_own_fields_in_modules_and_functions() {
    let source = "struct First { pub source_path: Option<String> }
        mod nested { struct Second { pub output_file: Option<String> } }
        fn execute() { struct Local { pub values_path: Option<String> } }
        struct Last { pub text_path: Option<String> }";
    assert_eq!(
        path_params(source),
        [
            ("First".to_owned(), "source_path".to_owned()),
            ("Second".to_owned(), "output_file".to_owned()),
            ("Local".to_owned(), "values_path".to_owned()),
            ("Last".to_owned(), "text_path".to_owned()),
        ]
    );
}

#[test]
fn only_public_named_optional_string_paths_are_scanned() {
    let source = "struct Params {
        source_path: Option<String>,
        pub(crate) crate_path: Option<String>,
        pub(super) parent_path: Option<String>,
        pub required_path: String,
        pub number_path: Option<u32>,
        pub nested_path: Option<Option<String>>,
        pub qualified_path: std::option::Option<String>,
        pub qualified_string_path: Option<std::string::String>,
        pub query: Option<String>,
        pub output_file: Option /* type comment */ < String >,
    }
    struct Tuple(pub Option<String>);
    enum Other { Named { output_file: Option<String> } }";
    assert_eq!(
        path_params(source),
        [("Params".to_owned(), "output_file".to_owned())]
    );
}

#[test]
fn removed_allowlist_entries_are_reported_as_stale() {
    let found: BTreeSet<_> = POLICY_CHECKED_PARAMS
        .iter()
        .map(|&(file, owner, field)| (file.to_owned(), owner.to_owned(), field.to_owned()))
        .collect();
    assert!(stale_params(&found).is_empty());
    for entry in &found {
        let mut remaining = found.clone();
        remaining.remove(entry);
        assert_eq!(
            stale_params(&remaining),
            [format!("{}: {}.{}", entry.0, entry.1, entry.2)]
        );
    }
}

#[test]
fn path_fields_use_the_shared_fail_closed_boundary() {
    let prefix = "struct NewParams { pub source_path: Option<String> }";
    let source = format!(
        "{prefix}
#[cfg( /* boundary */ test )] mod tests {{
            struct Fixture {{ pub fixture_path: Option<String> }}
        }}"
    );
    assert_eq!(path_params(&source), path_params(prefix));
    assert!(
        path_params("#![cfg(test)] struct Fixture { pub fixture_path: Option<String> }").is_empty()
    );
    for source in [
        "#[cfg(test)] fn helper() {} struct NewParams { pub source_path: Option<String> }",
        "#[cfg(test)] mod tests {} struct NewParams { pub output_file: Option<String> }",
        "struct NewParams { pub output_file: Option<String>",
    ] {
        assert!(
            std::panic::catch_unwind(|| path_params(source)).is_err(),
            "{source}"
        );
    }
}

#[test]
fn direct_filesystem_access_is_detected() {
    for source in [
        "std::fs::write(path, bytes)",
        "std :: fs :: read (path)",
        "std::fs::read_to_string(path)",
        "fs::read_dir(path)",
        "tokio::fs::read(path).await",
        "File :: open (path)",
        "std::fs::File::open(path)",
        "use std::fs::OpenOptions;",
        "OpenOptions::new().write(true).open(path)",
    ] {
        let source = format!("fn production() {{ {source}; }}");
        assert!(!direct_filesystem_calls(&source).is_empty(), "{source}");
    }
}

#[test]
fn additional_filesystem_calls_name_the_tool_and_policy() {
    for namespace in ["fs", "std::fs", "tokio::fs"] {
        for operation in [
            "write",
            "read",
            "read_to_string",
            "read_dir",
            "copy",
            "rename",
            "remove_file",
            "remove_dir",
            "remove_dir_all",
        ] {
            assert_filesystem_rejected(&format!("{namespace}::{operation}(path, output)"));
        }
    }
    for namespace in ["File", "std::fs::File", "tokio::fs::File"] {
        for operation in ["open", "create", "create_new"] {
            assert_filesystem_rejected(&format!("{namespace}::{operation}(path)"));
        }
    }
    assert_filesystem_rejected("tokio :: fs :: File :: create_new (path).await");
    assert_filesystem_rejected("std :: fs :: copy (source, output)");
}

fn assert_filesystem_rejected(source: &str) {
    let source = format!("fn production() {{ {source}; }}");
    let errors = filesystem_policy_errors("gmail_tools.rs", &source);
    assert!(!errors.is_empty(), "{source}");
    for error in errors {
        assert!(error.contains("gmail_tools.rs"), "{error}");
        assert!(error.contains("PathPolicy"), "{error}");
    }
}

#[test]
fn imported_filesystem_functions_are_rejected() {
    for namespace in ["std", "tokio"] {
        for operation in [
            "write",
            "read",
            "read_to_string",
            "read_dir",
            "copy",
            "rename",
            "remove_file",
            "remove_dir",
            "remove_dir_all",
            "*",
        ] {
            for tree in [
                format!("{namespace}::fs::{operation}"),
                format!("{namespace}::fs::{{self, {operation}, File}}"),
                format!("{namespace}::{{path::Path, fs::{{File, {operation}, self}}}}"),
                format!("{{{namespace}::fs::{operation}, other::Thing}}"),
            ] {
                assert_filesystem_rejected(&format!("use {tree};"));
            }
        }
        assert_filesystem_rejected(&format!(
            "use {namespace}::fs::read_to_string as load; load(path);"
        ));
        assert_filesystem_rejected(&format!(
            "use {namespace}::{{ fs :: {{ read as load, write as save, }}, }};"
        ));
    }
    assert_filesystem_rejected("use ::std :: fs :: read_to_string; read_to_string(path);");
    assert_filesystem_rejected(
        "use std::{\n path::Path,\n fs::{\n read_to_string as load,\n },\n};",
    );
}

#[test]
fn renamed_filesystem_modules_and_files_are_rejected() {
    for namespace in ["std", "tokio"] {
        for tree in [
            format!("{namespace}::fs as disk"),
            format!("{namespace}::fs::File as LocalFile"),
            format!("{namespace}::fs::{{self as disk, File as LocalFile}}"),
            format!("{namespace}::fs::{{File, self as disk}}"),
            format!("{namespace}::fs::{{self, File as LocalFile}}"),
            format!("{namespace}::{{path::Path, fs as disk}}"),
            format!("{namespace}::{{path::Path, fs::{{self as disk, File as LocalFile,}}}}"),
            format!("{{{namespace}::fs as disk, other::Thing}}"),
            format!("{{{namespace}::fs::File as LocalFile, other::Thing}}"),
            format!("::{namespace} :: fs as disk"),
            format!("::{namespace} :: fs :: File as LocalFile"),
            format!("{namespace} :: {{ fs :: {{ self\n as disk, File\n as LocalFile, }}, }}"),
            format!("{namespace}::fs as fs"),
            format!("{namespace}::fs::File as File"),
        ] {
            assert_filesystem_rejected(&format!("use {tree};"));
        }
        for source in [
            format!("use {namespace}::fs as disk; disk::write(path, bytes);"),
            format!("use {namespace}::fs::File as LocalFile; LocalFile::create(path).await;"),
            format!("use {namespace}::fs::{{self as disk, File as LocalFile}}; disk::copy(source, output); LocalFile::open(path);"),
        ] {
            assert_filesystem_rejected(&source);
        }
    }
}

#[test]
fn allowlisted_parameter_does_not_allow_aliased_filesystem_access() {
    for namespace in ["std", "tokio"] {
        for (import, call) in [
            (
                format!("{namespace}::fs as disk"),
                "disk::write(params.output_file, bytes)",
            ),
            (
                format!("{namespace}::fs::File as LocalFile"),
                "LocalFile::create(params.output_file)",
            ),
            (
                format!("{namespace}::fs::{{self as disk, File as LocalFile}}"),
                "disk::copy(source, params.output_file)",
            ),
        ] {
            let source = format!(
                "use {import};
                pub struct GmailMessageReadParams {{ pub output_file: Option<String> }}
                fn execute(params: GmailMessageReadParams) {{ {call}; }}"
            );
            assert!(unchecked_params("gmail_tools.rs", &source).is_empty());
            let errors = filesystem_policy_errors("gmail_tools.rs", &source);
            assert_eq!(errors.len(), 1, "{source}");
            assert!(errors[0].contains(&format!("use {import};")));
            assert!(errors[0].contains("gmail_tools.rs"));
            assert!(errors[0].contains("PathPolicy"));
        }
    }
}

#[test]
fn allowlisted_parameter_does_not_allow_direct_filesystem_access() {
    let source = "pub struct GmailMessageReadParams { pub output_file: Option<String> }
        fn execute(params: GmailMessageReadParams) { std::fs::File::create(params.output_file); }";
    assert!(unchecked_params("gmail_tools.rs", source).is_empty());
    assert_filesystem_rejected(source);
}

#[test]
fn module_and_unrelated_function_imports_are_permitted() {
    for source in [
        "use std::fs; use tokio::fs::File;",
        "use tokio::{fs::{self, File},}; use ::std::fs::File;",
        "use other::fs as disk; use other::fs::File as LocalFile;",
        "use std::path as paths; use tokio::io as local_io;",
        "use std::fs::{metadata as stat, canonicalize as resolve};",
        "use std::{fs::{self, File}, path::Path};",
        "use other::{read, write, copy, rename, remove_file};",
        "use std::fs::{metadata, canonicalize};",
        "PathPolicy::check_read(path); check_output_file(path);",
    ] {
        let source = format!("fn production() {{ {source}; }}");
        assert!(direct_filesystem_calls(&source).is_empty(), "{source}");
    }
}

#[test]
fn test_fixture_io_is_excluded() {
    let source = "pub struct Params { pub query: Option<String> }
        #[cfg(test)]
        mod tests {
            pub struct Fixture { pub fixture_path: Option<String> }
            use std::fs::{read_to_string, copy};
            use std::fs as disk;
            use tokio::fs::{self as async_disk, File as LocalFile};
            fn fixture() {
                std::fs::write(path, bytes);
                tokio::fs::File::create(path);
                std::fs::remove_file(path);
                disk::write(path, bytes);
                async_disk::copy(source, output);
                LocalFile::create(path).await;
            }
        }";
    assert!(path_params(source).is_empty());
    assert!(direct_filesystem_calls(source).is_empty());
}

#[test]
fn boundary_text_cannot_hide_new_paths_or_filesystem_calls() {
    let source = r##"
        // #[cfg(test)]
        const TEXT: &str = "#[cfg(test)]";
        pub struct NewParams { pub source_path: Option<String> }
        fn production() { std::fs::write(path, bytes); }
        #[cfg(test)]
        mod tests { fn fixture() { std::fs::read(path); } }
    "##;
    assert_eq!(unchecked_params("new_tools.rs", source).len(), 1);
    assert_eq!(direct_filesystem_calls(source), ["std::fs::write("]);
}

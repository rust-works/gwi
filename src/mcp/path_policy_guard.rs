//! Source tripwires for caller-chosen MCP paths; not a Rust parser or alias analysis.
//!
//! Covers fs reads/writes/copy/rename/removal, File open/create/create_new, and
//! OpenOptions with std/tokio or imported fs/File spellings. Direct imports of
//! guarded functions (including renamed, nested/grouped and glob imports) are
//! rejected at the import. Module/type aliases, macros, and comments/strings are
//! not resolved. Only tool files are scanned; policy helpers and trailing tests
//! remain outside the production scan.

use std::collections::BTreeSet;
use std::path::Path;

use regex::Regex;

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

fn production(source: &str) -> &str {
    // Match the existing mutation-wrapper guard and the trailing test-module convention.
    source.split("#[cfg(test)]").next().unwrap()
}

fn path_params(source: &str) -> Vec<(String, String)> {
    let structs = Regex::new(r"\bstruct\s+(\w+)\s*\{").unwrap();
    let fields = Regex::new(r"pub\s+(\w+)\s*:\s*Option\s*<\s*String\s*>").unwrap();
    let source = production(source);
    fields
        .captures_iter(source)
        .filter(|field| &field[1] == "output_file" || field[1].ends_with("_path"))
        .map(|field| {
            let before = &source[..field.get(0).unwrap().start()];
            let owner = structs.captures_iter(before).last().unwrap();
            (owner[1].to_owned(), field[1].to_owned())
        })
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

// Expand conventional use trees without resolving aliases. Renamed function imports
// are checked by their original path, so the local name cannot hide direct I/O.
fn imported_paths(tree: &str, prefix: &str, paths: &mut Vec<String>) {
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
        let original = tree.split_whitespace().take_while(|part| *part != "as");
        paths.push(format!("{prefix}{}", original.collect::<String>()));
    }
}

fn direct_filesystem_calls(source: &str) -> Vec<String> {
    // Retain read* coverage (including read_dir), plus operations that can replace
    // or remove caller-chosen files. OpenOptions also catches aliased builders.
    let calls = Regex::new(
        r"\b(?:(?:(?:std|tokio)\s*::\s*)?fs\s*::\s*(?:write|read\w*|copy|rename|remove_file|remove_dir|remove_dir_all)\s*\(|File\s*::\s*(?:open|create|create_new)\s*\(|OpenOptions\b)",
    )
    .unwrap();
    let source = production(source);
    let mut found: Vec<_> = calls
        .find_iter(source)
        .map(|call| call.as_str().to_owned())
        .collect();
    let imports = Regex::new(r"\buse\s+([^;]+);").unwrap();
    let forbidden = Regex::new(
        r"^(?:std|tokio)::fs::(?:write|read\w*|copy|rename|remove_file|remove_dir|remove_dir_all|\*)$",
    )
    .unwrap();
    for import in imports.captures_iter(source) {
        let mut paths = Vec::new();
        imported_paths(&import[1], "", &mut paths);
        if paths
            .iter()
            .any(|path| forbidden.is_match(path.trim_start_matches("::")))
        {
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
        let unchecked = unchecked_params(file, &source);
        assert!(unchecked.is_empty(), "{}", unchecked.join("\n"));
        for (owner, field) in path_params(&source) {
            found.insert((file.to_owned(), owner, field));
        }
        let errors = filesystem_policy_errors(file, &source);
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }
    let expected = POLICY_CHECKED_PARAMS
        .iter()
        .map(|&(file, owner, field)| (file.to_owned(), owner.to_owned(), field.to_owned()))
        .collect();
    assert_eq!(found, expected, "update stale PathPolicy allowlist entries");
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
        assert!(!direct_filesystem_calls(source).is_empty(), "{source}");
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
    let errors = filesystem_policy_errors("gmail_tools.rs", source);
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
        "use std::{fs::{self, File}, path::Path};",
        "use other::{read, write, copy, rename, remove_file};",
        "use std::fs::{metadata, canonicalize};",
        "PathPolicy::check_read(path); check_output_file(path);",
    ] {
        assert!(direct_filesystem_calls(source).is_empty(), "{source}");
    }
}

#[test]
fn test_fixture_io_is_excluded() {
    let source = "pub struct Params { pub query: Option<String> }
        #[cfg(test)]
        mod tests {
            pub struct Fixture { pub fixture_path: Option<String> }
            use std::fs::{read_to_string, copy};
            fn fixture() {
                std::fs::write(path, bytes);
                tokio::fs::File::create(path);
                std::fs::remove_file(path);
            }
        }";
    assert!(path_params(source).is_empty());
    assert!(direct_filesystem_calls(source).is_empty());
}

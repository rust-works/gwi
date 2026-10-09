//! Source tripwires for caller-chosen MCP paths; not a Rust parser or alias analysis.

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

fn direct_filesystem_calls(source: &str) -> Vec<String> {
    // Include imported fs/File spellings and async fs, and tolerate formatting. Banning
    // OpenOptions itself also catches imported/aliased builders before a method call.
    let calls = Regex::new(
        r"\b(?:(?:(?:std|tokio)\s*::\s*)?fs\s*::\s*(?:write|read\w*)\s*\(|File\s*::\s*open\s*\(|OpenOptions\b)",
    )
    .unwrap();
    calls
        .find_iter(production(source))
        .map(|call| call.as_str().to_owned())
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
        let calls = direct_filesystem_calls(&source);
        assert!(
            calls.is_empty(),
            "{file}: direct filesystem access {calls:?}; use the PathPolicy helpers in content_input.rs or output_file.rs"
        );
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
fn test_fixture_io_is_excluded() {
    let source = "pub struct Params { pub query: Option<String> }
        #[cfg(test)]
        mod tests {
            pub struct Fixture { pub fixture_path: Option<String> }
            fn fixture() { std::fs::write(path, bytes); }
        }";
    assert!(path_params(source).is_empty());
    assert!(direct_filesystem_calls(source).is_empty());
}

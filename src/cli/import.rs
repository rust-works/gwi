//! `gwi import`: copy Google Workspace settings and the Drive lease ledger out of omni-dev.
//!
//! gwi owns `~/.gwi/settings.json` and `<state_dir>/gwi/lease-ledger.jsonl` (ADR-0001) and
//! does not read omni-dev's `~/.omni-dev/settings.json` or its lease ledger. This command
//! is the bridge for someone who already configured Gmail or Drive there: it copies the
//! relevant items across so they can switch without logging in again. The settings are
//! handled here; the lease ledger is handled in the `ledger` submodule.
//!
//! The rules that make it safe to run, and to run twice:
//!
//! - It **copies**; the source file is only ever read.
//! - It merges at the level of one item (an account, the default-account name, one
//!   environment variable), so importing never clobbers unrelated gwi settings.
//! - An item gwi already has with a *different* value is a conflict and is left
//!   alone unless `--force` is given. An identical one is reported as unchanged,
//!   so a second run changes nothing.
//! - The report names items and paths, **never values**: accounts hold client
//!   secrets and refresh tokens.
//! - The file is written through the same hardened writer as every other settings
//!   change (`0700` directory, `0600` file).
//!
//! The settings are handled as raw JSON, not through gwi's typed `Settings`, so the
//! Drive and lease blocks (which gwi does not model yet) and any field a newer
//! omni-dev added come across verbatim.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use serde_json::{Map, Value};

use crate::utils::settings::{read_settings_value, write_settings_value, Settings};

mod ledger;

/// Settings `env` keys with these prefixes belong to Google Workspace and are
/// imported; every other omni-dev credential (Atlassian, Datadog, ...) stays behind.
const GOOGLE_ENV_PREFIXES: &[&str] = &["GMAIL_", "DRIVE_"];

/// The `mcp` settings gwi reads, which are imported.
const MCP_KEYS: &[&str] = &["log_level", "max_response_bytes"];

/// omni-dev variable prefixes whose gwi spelling differs, so an imported
/// `OMNI_DEV_GMAIL_ACCOUNT` keeps working as `GWI_GMAIL_ACCOUNT`.
const RENAMED_ENV_PREFIXES: &[(&str, &str)] = &[
    ("OMNI_DEV_GMAIL_", "GWI_GMAIL_"),
    ("OMNI_DEV_DRIVE_", "GWI_DRIVE_"),
];

/// Import Google Workspace settings and the Drive lease ledger from omni-dev.
#[derive(Parser)]
pub struct ImportCommand {
    /// The omni-dev settings file to read.
    ///
    /// Defaults to `~/.omni-dev/settings.json`. It is only read, never changed.
    #[arg(long, value_name = "PATH")]
    pub source: Option<PathBuf>,

    /// The omni-dev lease ledger to read.
    ///
    /// Defaults to `omni-dev/lease-ledger.jsonl` in the state directory. It is only read,
    /// never changed; a missing ledger is not an error.
    #[arg(long, value_name = "PATH")]
    pub source_ledger: Option<PathBuf>,

    /// Show what would be imported and write nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// Overwrite items and leases that gwi already has with a different value.
    ///
    /// Without it such items are reported as conflicts and left as they are.
    #[arg(long)]
    pub force: bool,
}

impl ImportCommand {
    /// Runs the import against the real settings and ledger files.
    ///
    /// The settings and the ledger are independent: once the paths are resolved both
    /// always run, and the command fails if either one reports a conflict or an error.
    pub fn execute(self) -> Result<()> {
        let source = match self.source {
            Some(path) => path,
            None => default_source()?,
        };
        let target = Settings::get_settings_path()?;
        let source_ledger = match self.source_ledger {
            Some(path) => path,
            None => ledger::default_source_ledger()?,
        };
        let target_ledger = crate::drive::lease::ledger::ledger_path()?;
        run_all(
            &Paths {
                source: &source,
                target: &target,
                source_ledger: &source_ledger,
                target_ledger: &target_ledger,
            },
            self.dry_run,
            self.force,
            &mut std::io::stdout(),
        )
    }
}

/// The four files an import reads and writes.
struct Paths<'a> {
    source: &'a Path,
    target: &'a Path,
    source_ledger: &'a Path,
    target_ledger: &'a Path,
}

/// Imports the settings and then the lease ledger, reporting both to `out`.
///
/// A missing omni-dev settings file is an error only when there is no lease ledger to
/// import either: someone who kept Drive in the environment has a ledger and no settings.
fn run_all(paths: &Paths<'_>, dry_run: bool, force: bool, out: &mut impl Write) -> Result<()> {
    let settings = if !paths.source.exists() && paths.source_ledger.exists() {
        writeln!(
            out,
            "No omni-dev settings file at {}; importing the lease ledger only.",
            paths.source.display()
        )
        .map_err(Into::into)
    } else {
        run_import(paths.source, paths.target, dry_run, force, out)
    };
    writeln!(out)?;
    let leases = ledger::run_ledger_import(
        paths.source_ledger,
        paths.target_ledger,
        dry_run,
        force,
        out,
    );
    match (settings, leases) {
        (Err(settings), Err(leases)) => Err(anyhow!("{settings:#}\n{leases:#}")),
        (Err(err), Ok(())) | (Ok(()), Err(err)) => Err(err),
        (Ok(()), Ok(())) => Ok(()),
    }
}

/// omni-dev's settings file: `~/.omni-dev/settings.json`.
fn default_source() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Failed to determine home directory")?;
    Ok(home.join(".omni-dev").join("settings.json"))
}

/// What happened to one imported item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    /// gwi had no such item; it was (or, in a dry run, would be) added.
    Added,
    /// gwi already has exactly this value.
    Unchanged,
    /// gwi has a different value and `--force` was not given.
    Conflict,
    /// gwi had a different value and `--force` replaced it.
    Overwritten,
    /// A lease ledger row gwi still had live was released, because omni-dev had since
    /// released it and nothing else about the row differs.
    Released,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Unchanged => "unchanged",
            Self::Conflict => "conflict",
            Self::Overwritten => "overwritten",
            Self::Released => "released",
        }
    }
}

/// One item selected from the source, addressed by its destination path.
#[derive(Debug)]
struct Item {
    /// Where it lands in gwi's settings, e.g. `["gmail", "accounts", "work"]`.
    path: Vec<String>,
    /// The key it had in omni-dev when that differs (a renamed variable).
    renamed_from: Option<String>,
    value: Value,
}

impl Item {
    fn display_path(&self) -> String {
        self.path.join(".")
    }
}

/// The outcome of merging the items into a settings value.
#[derive(Debug)]
struct Merged {
    /// The settings after the merge (equal to the input when nothing changed).
    value: Value,
    /// Each item with what happened to it, in a stable order.
    results: Vec<(Item, Status)>,
    /// Things the user must act on that the import cannot do for them.
    warnings: Vec<String>,
}

impl Merged {
    fn count(&self, status: Status) -> usize {
        self.results.iter().filter(|(_, s)| *s == status).count()
    }

    /// Whether the merge changed the settings.
    fn changed(&self) -> bool {
        self.count(Status::Added) + self.count(Status::Overwritten) > 0
    }
}

/// Selects what to import from omni-dev's settings.
///
/// Accounts and the other keys of the `gmail` and `drive` blocks are items of their
/// own, as are the `lease` block and each Google environment variable (in the base
/// `env` and in every profile's `env`).
fn select_items(source: &Value) -> Vec<Item> {
    let mut items = Vec::new();

    for block in ["gmail", "drive"] {
        let Some(map) = source.get(block).and_then(Value::as_object) else {
            continue;
        };
        for (key, value) in map {
            if key == "accounts" {
                if let Some(accounts) = value.as_object() {
                    for (name, account) in accounts {
                        items.push(Item {
                            path: vec![block.into(), "accounts".into(), name.clone()],
                            renamed_from: None,
                            value: account.clone(),
                        });
                    }
                }
            } else {
                items.push(Item {
                    path: vec![block.into(), key.clone()],
                    renamed_from: None,
                    value: value.clone(),
                });
            }
        }
    }

    // The two `mcp` defaults `gwi-mcp` reads. omni-dev's other MCP setting, the AI
    // `default_model`, has no use here and stays behind.
    if let Some(mcp) = source.get("mcp").and_then(Value::as_object) {
        for key in MCP_KEYS {
            if let Some(value) = mcp.get(*key) {
                items.push(Item {
                    path: vec!["mcp".into(), (*key).into()],
                    renamed_from: None,
                    value: value.clone(),
                });
            }
        }
    }

    if let Some(lease) = source.get("lease") {
        items.push(Item {
            path: vec!["lease".into()],
            renamed_from: None,
            value: lease.clone(),
        });
    }

    env_items(source.get("env"), &["env".into()], &mut items);
    if let Some(profiles) = source.get("profiles").and_then(Value::as_object) {
        for (name, profile) in profiles {
            env_items(
                profile.get("env"),
                &["profiles".into(), name.clone(), "env".into()],
                &mut items,
            );
        }
    }
    items
}

/// Appends an item per Google variable in `env`, renaming omni-dev's own prefixes.
fn env_items(env: Option<&Value>, prefix: &[String], items: &mut Vec<Item>) {
    let Some(map) = env.and_then(Value::as_object) else {
        return;
    };
    for (key, value) in map {
        let (name, renamed_from) = match RENAMED_ENV_PREFIXES
            .iter()
            .find_map(|(old, new)| key.strip_prefix(old).map(|rest| format!("{new}{rest}")))
        {
            Some(renamed) => (renamed, Some(key.clone())),
            None if GOOGLE_ENV_PREFIXES.iter().any(|p| key.starts_with(p)) => (key.clone(), None),
            None => continue,
        };
        let mut path = prefix.to_vec();
        path.push(name);
        items.push(Item {
            path,
            renamed_from,
            value: value.clone(),
        });
    }
}

/// Looks up `path` in `root`.
fn lookup<'a>(root: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(root, |node, key| node.get(key))
}

/// The object map of `node`, replacing a non-object node with an empty object.
fn object_mut(node: &mut Value) -> &mut Map<String, Value> {
    if !node.is_object() {
        *node = Value::Object(Map::new());
    }
    match node {
        Value::Object(map) => map,
        _ => unreachable!("replaced with an object above"),
    }
}

/// Sets `path` in `root` to `value`, creating (or replacing non-object) parents.
fn set_path(root: &mut Value, path: &[String], value: Value) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut node = root;
    for key in parents {
        node = object_mut(node)
            .entry(key.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    object_mut(node).insert(last.clone(), value);
}

/// Merges `items` into `target`, never overwriting a differing value unless `force`.
///
/// Pure: the source directory is only used to recognise references that point back
/// into it.
fn merge(target: &Value, items: Vec<Item>, force: bool, source_dir: Option<&Path>) -> Merged {
    let mut value = target.clone();
    let mut results = Vec::new();
    let mut warnings = Vec::new();

    for item in items {
        let status = match lookup(&value, &item.path) {
            None => Status::Added,
            Some(current) if *current == item.value => Status::Unchanged,
            Some(_) if force => Status::Overwritten,
            Some(_) => Status::Conflict,
        };
        if matches!(status, Status::Added | Status::Overwritten) {
            warnings.extend(reference_warnings(&item, source_dir));
            set_path(&mut value, &item.path, item.value.clone());
        }
        results.push((item, status));
    }
    Merged {
        value,
        results,
        warnings,
    }
}

/// Warnings about `*_file` references in an imported item that will not resolve the
/// way they did in omni-dev: a path inside the omni-dev directory (which the user may
/// remove) or a relative path (which resolves against the working directory).
fn reference_warnings(item: &Item, source_dir: Option<&Path>) -> Vec<String> {
    let Some(map) = item.value.as_object() else {
        return Vec::new();
    };
    let mut warnings = Vec::new();
    for (key, value) in map {
        let Some(path) = value.as_str().filter(|_| key.ends_with("_file")) else {
            continue;
        };
        let at = format!("{}.{key}", item.display_path());
        if source_dir.is_some_and(|dir| Path::new(path).starts_with(dir)) {
            warnings.push(format!(
                "{at} points inside the omni-dev directory ({path}); move that file somewhere \
                 stable and update the path before removing omni-dev's files"
            ));
        } else if Path::new(path).is_relative() {
            warnings.push(format!(
                "{at} is a relative path ({path}), which resolves against the directory gwi \
                 runs in"
            ));
        }
    }
    warnings
}

/// Imports from `source` into `target`, writing the report to `out`.
///
/// Returns an error when the source is unusable, or when conflicts were left
/// unresolved (after importing everything that could be imported safely).
fn run_import(
    source: &Path,
    target: &Path,
    dry_run: bool,
    force: bool,
    out: &mut impl Write,
) -> Result<()> {
    if !source.exists() {
        bail!(
            "no omni-dev settings file at {}; pass --source to point at one",
            source.display()
        );
    }
    let source_value = read_settings_value(source)?;
    let items = select_items(&source_value);
    writeln!(
        out,
        "Importing from {} into {}",
        source.display(),
        target.display()
    )?;
    if items.is_empty() {
        writeln!(
            out,
            "Nothing to import: no Gmail or Drive settings found in the source."
        )?;
        return Ok(());
    }

    let target_value = read_settings_value(target)?;
    let merged = merge(&target_value, items, force, source.parent());

    for (item, status) in &merged.results {
        let note = match (&item.renamed_from, status) {
            (Some(old), _) => format!("  (renamed from {old})"),
            (None, Status::Conflict) => "  (gwi has a different value; use --force)".to_string(),
            _ => String::new(),
        };
        writeln!(
            out,
            "  {:<11} {}{note}",
            status.label(),
            item.display_path()
        )?;
    }
    for warning in &merged.warnings {
        writeln!(out, "warning: {warning}")?;
    }

    let conflicts = merged.count(Status::Conflict);
    writeln!(
        out,
        "{}{} added, {} overwritten, {} unchanged, {} conflict(s).",
        if dry_run {
            "Dry run, nothing written: "
        } else {
            ""
        },
        merged.count(Status::Added),
        merged.count(Status::Overwritten),
        merged.count(Status::Unchanged),
        conflicts,
    )?;

    if !dry_run && merged.changed() {
        write_settings_value(target, &merged.value)?;
    }
    if conflicts > 0 {
        return Err(anyhow!(
            "{conflicts} item(s) were not imported because gwi already has a different value; \
             re-run with --force to overwrite them"
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A secret that must never appear in any report or error.
    const SECRET: &str = "s3cr3t-refresh-token-value";

    /// A settings file shaped like one omni-dev wrote for a Gmail and Drive user.
    fn omni_dev_settings() -> Value {
        json!({
            "env": {
                "GMAIL_CLIENT_ID": "base-id",
                "ATLASSIAN_API_TOKEN": "not-google",
                "OMNI_DEV_GMAIL_ACCOUNT": "work",
                "DATADOG_API_KEY": "not-google"
            },
            "profiles": {
                "home": {"env": {"DRIVE_CLIENT_ID": "home-id", "SNOWFLAKE_TOKEN": "not-google"}}
            },
            "gmail": {
                "default_account": "work",
                "accounts": {
                    "work": {
                        "client_id": "id",
                        "refresh_token": SECRET,
                        "scope": "gmail.readonly",
                        "a_field_from_a_newer_omni_dev": {"nested": [1, 2]}
                    }
                }
            },
            "drive": {
                "accounts": {"work": {"client_id": "id", "backup_folder_id": "backup1"}}
            },
            "lease": {"expiry_minutes": 15},
            "mcp": {"log_level": "info", "max_response_bytes": 2048, "default_model": "not-for-gwi"}
        })
    }

    fn paths(items: &[Item]) -> Vec<String> {
        items.iter().map(Item::display_path).collect()
    }

    fn write_json(path: &Path, value: &Value) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// A source and a target path in a fresh directory.
    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join(".omni-dev").join("settings.json");
        let target = dir.path().join(".gwi").join("settings.json");
        write_json(&source, &omni_dev_settings());
        (dir, source, target)
    }

    fn import(source: &Path, target: &Path, dry_run: bool, force: bool) -> (Result<()>, String) {
        let mut out = Vec::new();
        let result = run_import(source, target, dry_run, force, &mut out);
        (result, String::from_utf8(out).unwrap())
    }

    #[test]
    fn selects_google_items_and_leaves_other_products_behind() {
        let items = select_items(&omni_dev_settings());
        let selected = paths(&items);

        for expected in [
            "gmail.default_account",
            "gmail.accounts.work",
            "drive.accounts.work",
            "lease",
            "env.GMAIL_CLIENT_ID",
            "env.GWI_GMAIL_ACCOUNT",
            "profiles.home.env.DRIVE_CLIENT_ID",
        ] {
            assert!(selected.contains(&expected.to_string()), "{selected:?}");
        }
        for expected in ["mcp.log_level", "mcp.max_response_bytes"] {
            assert!(selected.contains(&expected.to_string()), "{selected:?}");
        }
        for foreign in ["ATLASSIAN", "DATADOG", "SNOWFLAKE", "default_model"] {
            assert!(
                selected.iter().all(|p| !p.contains(foreign)),
                "{foreign} must not be imported: {selected:?}"
            );
        }
    }

    #[test]
    fn omni_dev_account_variables_are_renamed_to_gwis() {
        let items = select_items(&omni_dev_settings());
        let renamed = items
            .iter()
            .find(|i| i.display_path() == "env.GWI_GMAIL_ACCOUNT")
            .unwrap();

        assert_eq!(
            renamed.renamed_from.as_deref(),
            Some("OMNI_DEV_GMAIL_ACCOUNT")
        );
        assert!(items
            .iter()
            .all(|i| !i.display_path().contains("OMNI_DEV_")));
    }

    #[test]
    fn imports_everything_into_an_empty_target_and_keeps_unknown_fields() {
        let (_dir, source, target) = setup();

        let (result, report) = import(&source, &target, false, false);

        result.unwrap();
        let written = read_json(&target);
        assert_eq!(written["gmail"]["default_account"], "work");
        assert_eq!(
            written["gmail"]["accounts"]["work"]["refresh_token"],
            SECRET
        );
        assert_eq!(
            written["gmail"]["accounts"]["work"]["a_field_from_a_newer_omni_dev"],
            json!({"nested": [1, 2]})
        );
        assert_eq!(
            written["drive"]["accounts"]["work"]["backup_folder_id"],
            "backup1"
        );
        assert_eq!(written["lease"]["expiry_minutes"], 15);
        assert_eq!(written["env"]["GMAIL_CLIENT_ID"], "base-id");
        assert_eq!(written["env"]["GWI_GMAIL_ACCOUNT"], "work");
        assert_eq!(
            written["profiles"]["home"]["env"]["DRIVE_CLIENT_ID"],
            "home-id"
        );
        assert!(written["env"].get("ATLASSIAN_API_TOKEN").is_none());
        assert!(
            report.contains("added       gmail.accounts.work"),
            "{report}"
        );
        assert!(
            report.contains("(renamed from OMNI_DEV_GMAIL_ACCOUNT)"),
            "{report}"
        );
    }

    #[test]
    fn the_report_and_errors_never_contain_a_secret() {
        let (_dir, source, target) = setup();
        write_json(
            &target,
            &json!({"gmail": {"accounts": {"work": {"refresh_token": "another-secret"}}}}),
        );

        let (result, report) = import(&source, &target, false, false);

        let error = result.unwrap_err().to_string();
        for text in [&report, &error] {
            assert!(
                !text.contains(SECRET) && !text.contains("another-secret"),
                "{text}"
            );
        }
    }

    #[test]
    fn unrelated_gwi_settings_survive_the_merge() {
        let (_dir, source, target) = setup();
        write_json(
            &target,
            &json!({
                "env": {"GMAIL_CLIENT_SECRET_FILE": "/keep/me"},
                "profiles": {"mine": {"env": {}}},
                "something_else": [1]
            }),
        );

        import(&source, &target, false, false).0.unwrap();

        let written = read_json(&target);
        assert_eq!(written["env"]["GMAIL_CLIENT_SECRET_FILE"], "/keep/me");
        assert_eq!(written["env"]["GMAIL_CLIENT_ID"], "base-id");
        assert!(written["profiles"].get("mine").is_some());
        assert_eq!(written["something_else"], json!([1]));
    }

    #[test]
    fn a_second_run_changes_nothing() {
        let (_dir, source, target) = setup();
        import(&source, &target, false, false).0.unwrap();
        let first = std::fs::read(&target).unwrap();

        let (result, report) = import(&source, &target, false, false);

        result.unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), first);
        assert!(!report.contains("added "), "{report}");
        assert!(report.contains("0 added, 0 overwritten"), "{report}");
    }

    #[test]
    fn a_differing_value_is_a_conflict_but_everything_else_still_imports() {
        let (_dir, source, target) = setup();
        write_json(
            &target,
            &json!({"gmail": {"accounts": {"work": {"client_id": "different"}}}}),
        );

        let (result, report) = import(&source, &target, false, false);

        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("1 item(s)") && error.contains("--force"),
            "{error}"
        );
        assert!(
            report.contains("conflict    gmail.accounts.work"),
            "{report}"
        );
        let written = read_json(&target);
        assert_eq!(
            written["gmail"]["accounts"]["work"]["client_id"],
            "different"
        );
        assert_eq!(written["drive"]["accounts"]["work"]["client_id"], "id");
    }

    #[test]
    fn force_overwrites_a_conflict() {
        let (_dir, source, target) = setup();
        write_json(
            &target,
            &json!({"gmail": {"accounts": {"work": {"client_id": "different"}}}}),
        );

        let (result, report) = import(&source, &target, false, true);

        result.unwrap();
        assert!(
            report.contains("overwritten gmail.accounts.work"),
            "{report}"
        );
        assert_eq!(
            read_json(&target)["gmail"]["accounts"]["work"]["client_id"],
            "id"
        );
    }

    #[test]
    fn a_dry_run_reports_but_writes_nothing() {
        let (_dir, source, target) = setup();

        let (result, report) = import(&source, &target, true, false);

        result.unwrap();
        assert!(!target.exists());
        assert!(report.contains("Dry run, nothing written"), "{report}");
        assert!(
            report.contains("added       gmail.accounts.work"),
            "{report}"
        );
    }

    #[test]
    fn the_source_is_never_modified() {
        let (_dir, source, target) = setup();
        let before = std::fs::read(&source).unwrap();

        import(&source, &target, false, true).0.unwrap();

        assert_eq!(std::fs::read(&source).unwrap(), before);
    }

    #[test]
    fn a_missing_source_names_the_path_and_the_flag() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.json");

        let error = import(&missing, &dir.path().join("t.json"), false, false)
            .0
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("nope.json") && error.contains("--source"),
            "{error}"
        );
    }

    #[test]
    fn an_unparseable_source_is_an_error_that_does_not_echo_its_contents() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("settings.json");
        std::fs::write(&source, format!("{{\"refresh_token\": \"{SECRET}\" oops")).unwrap();

        let error = format!(
            "{:#}",
            import(&source, &dir.path().join("t.json"), false, false)
                .0
                .unwrap_err()
        );

        assert!(error.contains("settings.json"), "{error}");
        assert!(!error.contains(SECRET), "{error}");
    }

    #[test]
    fn a_source_with_no_google_settings_imports_nothing_and_creates_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("settings.json");
        write_json(&source, &json!({"env": {"ATLASSIAN_API_TOKEN": "x"}}));
        let target = dir.path().join(".gwi").join("settings.json");

        let (result, report) = import(&source, &target, false, false);

        result.unwrap();
        assert!(report.contains("Nothing to import"), "{report}");
        assert!(!target.exists());
    }

    #[test]
    fn references_into_the_omni_dev_directory_and_relative_paths_are_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join(".omni-dev").join("settings.json");
        let inside = dir.path().join(".omni-dev").join("token");
        write_json(
            &source,
            &json!({"gmail": {"accounts": {
                "inside": {"refresh_token_file": inside.to_str().unwrap()},
                "relative": {"client_secret_file": "secrets/client.json"},
                "fine": {"refresh_token_file": "/etc/stable/token"}
            }}}),
        );
        let target = dir.path().join(".gwi").join("settings.json");

        let (result, report) = import(&source, &target, false, false);

        result.unwrap();
        assert!(
            report.contains(
                "gmail.accounts.inside.refresh_token_file points inside the omni-dev directory"
            ),
            "{report}"
        );
        assert!(
            report.contains("gmail.accounts.relative.client_secret_file is a relative path"),
            "{report}"
        );
        // The stable account is still imported, but no warning names one of its keys.
        assert!(!report.contains("accounts.fine."), "{report}");
    }

    #[cfg(unix)]
    #[test]
    fn the_written_settings_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, source, target) = setup();

        import(&source, &target, false, false).0.unwrap();

        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn set_path_replaces_a_non_object_parent() {
        let mut root = json!({"gmail": "not an object"});

        set_path(
            &mut root,
            &["gmail".to_string(), "accounts".to_string(), "a".to_string()],
            json!(1),
        );

        assert_eq!(root["gmail"]["accounts"]["a"], 1);
    }

    #[test]
    fn a_ledger_without_a_settings_file_still_imports_and_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = dir.path().join("omni-dev").join("lease-ledger.jsonl");
        std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
        std::fs::write(
            &ledger,
            r#"{"token":"t1","file_id":"f","version":"1","backup":{"kind":"drive_copy","file_id":"c"},"acquired_at":"2020-01-01T00:00:00Z","expires_at":"2020-01-01T00:15:00Z"}"#,
        )
        .unwrap();
        let target_ledger = dir.path().join("gwi").join("lease-ledger.jsonl");
        let paths = Paths {
            source: &dir.path().join("none.json"),
            target: &dir.path().join("gwi").join("settings.json"),
            source_ledger: &ledger,
            target_ledger: &target_ledger,
        };
        let mut out = Vec::new();

        run_all(&paths, false, false, &mut out).unwrap();

        let report = String::from_utf8(out).unwrap();
        assert!(
            report.contains("importing the lease ledger only"),
            "{report}"
        );
        assert!(report.contains("added       lease t1"), "{report}");
        assert!(target_ledger.exists());
        assert!(!paths.target.exists(), "no settings file is invented");
    }

    #[test]
    fn nothing_to_import_from_either_source_is_still_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            source: &dir.path().join("none.json"),
            target: &dir.path().join("gwi").join("settings.json"),
            source_ledger: &dir.path().join("none.jsonl"),
            target_ledger: &dir.path().join("gwi").join("lease-ledger.jsonl"),
        };

        let err = run_all(&paths, false, false, &mut Vec::new()).unwrap_err();

        assert!(err.to_string().contains("--source"), "{err}");
    }

    #[test]
    fn both_halves_run_and_both_failures_are_reported() {
        let (dir, source, target) = setup();
        let ledger = dir.path().join("omni-dev").join("lease-ledger.jsonl");
        std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
        std::fs::write(&ledger, "not json\n").unwrap();
        write_json(
            &target,
            &json!({"gmail": {"accounts": {"work": {"client_id": "different"}}}}),
        );
        let paths = Paths {
            source: &source,
            target: &target,
            source_ledger: &ledger,
            target_ledger: &dir.path().join("gwi").join("lease-ledger.jsonl"),
        };

        let err = run_all(&paths, false, false, &mut Vec::new())
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("--force") && err.contains("lease ledger"),
            "{err}"
        );
        assert_eq!(
            read_json(&target)["drive"]["accounts"]["work"]["client_id"],
            "id",
            "the settings half still imported what it could"
        );
    }

    #[test]
    fn the_command_parses_its_flags() {
        let cmd = ImportCommand::try_parse_from([
            "import",
            "--source",
            "/tmp/s.json",
            "--dry-run",
            "--force",
        ])
        .unwrap();
        assert_eq!(cmd.source, Some(PathBuf::from("/tmp/s.json")));
        assert!(cmd.source_ledger.is_none());
        assert!(cmd.dry_run && cmd.force);

        let ledger =
            ImportCommand::try_parse_from(["import", "--source-ledger", "/tmp/l.jsonl"]).unwrap();
        assert_eq!(ledger.source_ledger, Some(PathBuf::from("/tmp/l.jsonl")));

        let defaults = ImportCommand::try_parse_from(["import"]).unwrap();
        assert!(defaults.source.is_none() && !defaults.dry_run && !defaults.force);
        assert!(defaults.source_ledger.is_none());
    }
}

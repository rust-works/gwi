//! `gwi log` — search and pretty-print the local invocation, HTTP and Drive-mutation log.
//!
//! Read-only and synchronous. Streams [`request_log::log_file_path`] and optional
//! numbered backups line by line, applies filters, and renders each match as `oneline`,
//! `json` (byte-identical to the on-disk NDJSON), or `full`.

mod format;
mod prune;
mod query;
mod stream;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use crate::request_log;
use crate::utils::env::{EnvSource, SystemEnv};
use query::Filter;

/// Output rendering for `gwi log`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum Format {
    /// One compact line per record (default).
    Oneline,
    /// The on-disk NDJSON line, verbatim (composes with `jq`).
    #[value(help = "The on-disk NDJSON line, verbatim (composes with jq)")]
    Json,
    /// A labelled, multi-line block per record.
    Full,
}

/// Searches and pretty-prints the local invocation + HTTP request log.
///
/// With no subcommand, the flags below search the log; the `prune` subcommand
/// trims the log to bound its on-disk growth.
#[derive(Parser)]
// A search flag placed before `prune` would otherwise parse and be silently
// ignored; refuse it instead. (`--profile` is global, so it is unaffected.)
#[command(args_conflicts_with_subcommands = true)]
pub struct LogCommand {
    /// Subcommand; when absent, the flags below search the log.
    // Keep first: its updater removes implicit defaults before search-field updates.
    #[command(flatten)]
    action: LogActions,
    /// Lower time bound: a relative window (`30m`, `2h`, `1d`), a date
    /// (`2026-07-01`), or an RFC3339 timestamp.
    #[arg(
        help = "Lower time bound: a relative window (30m, 2h, 1d), a date (2026-07-01), or an RFC3339 timestamp"
    )]
    #[arg(long, value_name = "DUR_OR_TS")]
    since: Option<String>,
    /// Upper time bound: same forms as `--since` (a relative value means that
    /// long ago). Pair with `--since` for a bounded window.
    #[arg(
        help = "Upper time bound: same forms as --since (a relative value means that long ago). Pair with --since for a bounded window"
    )]
    #[arg(long, value_name = "DUR_OR_TS")]
    until: Option<String>,
    /// Match the HTTP method (case-insensitive), e.g. `GET`.
    #[arg(help = "Match the HTTP method (case-insensitive), e.g. GET")]
    #[arg(long, value_name = "METHOD")]
    method: Option<String>,
    /// Match the status: exact (`200`), class (`5xx`), list (`4xx,5xx`),
    /// comparison (`>=400`), or a Drive-mutation status such as `blocked`
    /// (exact, case-insensitive; a comma list is accepted). Same values as
    /// `status:` in `--query`. A Drive-mutation status that no scanned record
    /// has draws a warning on stderr (a likely typo); the exit code is unchanged.
    #[arg(
        help = "Match the status: exact (200), class (5xx), list (4xx,5xx), comparison (>=400), or a Drive-mutation status such as blocked (exact, case-insensitive; a comma list is accepted). Same values as status: in --query. A Drive-mutation status that no scanned record has draws a warning on stderr (a likely typo); the exit code is unchanged"
    )]
    #[arg(long, value_name = "STATUS")]
    status: Option<String>,
    /// Match the service tag, e.g. `gmail`, `drive`.
    #[arg(help = "Match the service tag, e.g. gmail, drive")]
    #[arg(long, value_name = "NAME")]
    service: Option<String>,
    /// Match the resolved command path prefix, e.g. `"gmail read"`.
    #[arg(help = "Match the resolved command path prefix, e.g. \"gmail read\"")]
    #[arg(long, value_name = "PATH")]
    command: Option<String>,
    /// Match a substring of the request URL.
    #[arg(long, value_name = "SUBSTR")]
    url: Option<String>,
    /// Match a regular expression against the raw JSON line.
    #[arg(long, value_name = "REGEX")]
    grep: Option<String>,
    /// Require a fuzzy token (substring of the raw line); repeatable, AND-ed.
    #[arg(long, value_name = "TOKEN")]
    fuzzy: Vec<String>,
    /// A query expression (AND/OR/NOT, `field:value`, bare tokens); repeatable,
    /// AND-ed together. A field that is not built in is looked up in the
    /// record's context, and a warning names it if no record has it. Quote a
    /// word to search for it as text, e.g. `--query '"not"'`.
    #[arg(
        help = "A query expression (AND/OR/NOT, field:value, bare tokens); repeatable, AND-ed together. A field that is not built in is looked up in the record's context, and a warning names it if no record has it. Quote a word to search for it as text, e.g. --query '\"not\"'"
    )]
    #[arg(long, value_name = "EXPR")]
    query: Vec<String>,
    /// Match this record `id` or `invocation_id` (pulls a run and its requests).
    #[arg(help = "Match this record id or invocation_id (pulls a run and its requests)")]
    #[arg(long, value_name = "ID")]
    id: Option<String>,
    /// Output format.
    #[arg(short = 'o', long, value_enum, default_value_t = Format::Oneline)]
    output: Format,
    /// Deprecated: use `-o`/`--output` instead.
    #[arg(help = "Deprecated: use -o/--output instead")]
    #[arg(long = "format", hide = true)]
    format: Option<Format>,
    /// Show at most N (most recent) matching records.
    #[arg(short = 'n', long, value_name = "N")]
    limit: Option<usize>,
    /// Include numbered request-log backups, oldest first, before the live log.
    /// The limit applies across all files; follow then tails only the live log.
    #[arg(long, conflicts_with = "audit")]
    rotated: bool,
    /// Follow the log, printing new matching records as they are appended.
    #[arg(short = 'f', long)]
    follow: bool,
    /// Read the fail-closed audit log (`audit.jsonl`) instead of the
    /// best-effort request log — the leased-write lifecycle and refusal
    /// trail (ADR-0080 §11), not subject to
    /// `GWI_LOG_DISABLE`, rotation or `prune`. Every filter, the
    /// `--query` mini-language and all three output formats apply
    /// unchanged; only the file being read differs.
    #[arg(
        help = "Read the fail-closed audit log (audit.jsonl) instead of the best-effort request log — the leased-write lifecycle and refusal trail (ADR-0080 §11), not subject to GWI_LOG_DISABLE, rotation or prune. Every filter, the --query mini-language and all three output formats apply unchanged; only the file being read differs"
    )]
    #[arg(long)]
    audit: bool,
}

/// A `log` subcommand. Absent = search (the flags on [`LogCommand`]).
#[derive(Subcommand)]
enum LogAction {
    /// Prune old records to bound the log's on-disk growth.
    Prune(prune::PruneCommand),
}

/// Optional action parsing, including partial updates with no supplied action.
///
/// clap_derive 4.6.7 constructs an absent optional subcommand unconditionally
/// during updates. Flatten this adapter so search-only updates can retain None
/// instead of calling the enum constructor with no subcommand.
struct LogActions(Option<LogAction>);

impl clap::Args for LogActions {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        LogAction::augment_subcommands(cmd)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        LogAction::augment_subcommands_for_update(cmd)
    }
}

impl clap::FromArgMatches for LogActions {
    fn from_arg_matches(matches: &clap::ArgMatches) -> Result<Self, clap::Error> {
        Self::from_arg_matches_mut(&mut matches.clone())
    }

    fn from_arg_matches_mut(matches: &mut clap::ArgMatches) -> Result<Self, clap::Error> {
        let action = if matches.subcommand_name().is_some() {
            Some(LogAction::from_arg_matches_mut(matches)?)
        } else {
            None
        };
        Ok(Self(action))
    }

    fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
        self.update_from_arg_matches_mut(&mut matches.clone())
    }

    fn update_from_arg_matches_mut(
        &mut self,
        matches: &mut clap::ArgMatches,
    ) -> Result<(), clap::Error> {
        // This flattened field runs before the derived search-field updates.
        // Defaults are not supplied updates: keep stored flags and output.
        remove_default_bools(matches, &["rotated", "follow", "audit"]);
        if matches.value_source("output") == Some(clap::parser::ValueSource::DefaultValue) {
            matches.remove_one::<Format>("output");
        }
        match &mut self.0 {
            Some(LogAction::Prune(cmd)) => {
                // Prune is the only action. Delegate to its derived payload updater
                // after removing defaults from the supplied child matches.
                if let Some((_, mut child)) = matches.remove_subcommand() {
                    remove_default_bools(&mut child, &["dry_run", "audit"]);
                    cmd.update_from_arg_matches_mut(&mut child)?;
                }
            }
            None if matches.subcommand_name().is_some() => {
                self.0 = Some(LogAction::from_arg_matches_mut(matches)?);
            }
            None => {}
        }
        Ok(())
    }
}

/// Removes only implicit boolean values; explicit flags still update the state.
fn remove_default_bools(matches: &mut clap::ArgMatches, ids: &[&str]) {
    for id in ids {
        if matches.value_source(id) == Some(clap::parser::ValueSource::DefaultValue) {
            matches.remove_one::<bool>(id);
        }
    }
}

impl LogCommand {
    /// Executes the `gwi log` command.
    pub fn execute(mut self) -> Result<()> {
        if let Some(action) = self.action.0 {
            // `args_conflicts_with_subcommands` guarantees no search flag (so not
            // `--audit` either) reached here; `prune` has its own `--audit` that
            // refuses loudly.
            return match action {
                LogAction::Prune(cmd) => cmd.execute(),
            };
        }
        if let Some(format) = self.format.take() {
            eprintln!("warning: --format is deprecated; use -o/--output instead");
            self.output = format;
        }
        let path = self.resolve_path()?;
        let filter = Filter::build(query::FilterInput {
            since: self.since.as_deref(),
            until: self.until.as_deref(),
            method: self.method.as_deref(),
            status: self.status.as_deref(),
            service: self.service.as_deref(),
            command: self.command.as_deref(),
            url: self.url.as_deref(),
            grep: self.grep.as_deref(),
            fuzzy: &self.fuzzy,
            query: &self.query,
            id: self.id.as_deref(),
        })?;
        stream::run(
            &path,
            &filter,
            self.output,
            self.limit,
            self.follow,
            self.rotated,
        )
    }

    /// Resolves which file this invocation reads: `audit.jsonl` when
    /// `--audit` is set, else the ordinary `log.jsonl` — the entire effect
    /// of the flag (ADR-0080 §11).
    fn resolve_path(&self) -> Result<std::path::PathBuf> {
        self.resolve_path_with(&SystemEnv)
    }

    /// [`Self::resolve_path`], reading through an injected [`EnvSource`]
    /// (STYLE-0028).
    fn resolve_path_with(&self, env: &impl EnvSource) -> Result<std::path::PathBuf> {
        if self.audit {
            request_log::audit_file_path_with(env)
                .context("could not resolve the audit log file path")
        } else {
            request_log::log_file_path_with(env).context("could not resolve the log file path")
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::test_support::env::MapEnv;

    #[derive(Parser)]
    struct Wrapper {
        #[command(subcommand)]
        cmd: Wrapped,
    }

    #[derive(clap::Subcommand)]
    enum Wrapped {
        Log(LogCommand),
    }

    fn parse(args: &[&str]) -> LogCommand {
        let mut full = vec!["gwi", "log"];
        full.extend_from_slice(args);
        match Wrapper::try_parse_from(full).unwrap().cmd {
            Wrapped::Log(cmd) => cmd,
        }
    }

    #[test]
    fn search_only_cli_update_retains_unsupplied_state() {
        let mut cli = crate::Cli::try_parse_from([
            "gwi",
            "--profile",
            "work",
            "log",
            "--limit",
            "0",
            "--since",
            "2h",
            "--query",
            "status:5xx",
            "--fuzzy",
            "token",
            "--follow",
            "--audit",
            "--output",
            "json",
        ])
        .unwrap();
        cli.try_update_from(["gwi", "log", "--limit", "1"]).unwrap();
        assert_eq!(cli.profile.as_deref(), Some("work"));
        let crate::cli::Commands::Log(cmd) = cli.command else {
            panic!("expected log"); // patchcov: coverage ignore-line reason="guards this test's assumption; the parse above always yields Commands::Log for a log argv"
        };
        assert_eq!(cmd.limit, Some(1));
        assert_eq!(cmd.since.as_deref(), Some("2h"));
        assert_eq!(cmd.query, ["status:5xx"]);
        assert_eq!(cmd.fuzzy, ["token"]);
        assert!(cmd.follow);
        assert!(cmd.audit);
        assert_eq!(cmd.output, Format::Json);
    }

    fn log_from_cli(cli: crate::Cli) -> Box<LogCommand> {
        let crate::cli::Commands::Log(cmd) = cli.command else {
            panic!("expected log"); // patchcov: coverage ignore-line reason="guards this test helper; every caller passes a parsed log argv"
        };
        cmd
    }

    #[test]
    fn search_updates_accept_no_action_and_replace_supplied_lists() {
        let mut cli = crate::Cli::try_parse_from([
            "gwi", "log", "--query", "old", "--fuzzy", "old", "--limit", "0",
        ])
        .unwrap();
        cli.try_update_from(["gwi", "log"]).unwrap();
        cli.try_update_from(["gwi", "--profile", "work"]).unwrap();
        assert_eq!(cli.profile.as_deref(), Some("work"));
        cli.try_update_from([
            "gwi", "log", "--query", "new", "--query", "second", "--fuzzy", "new",
        ])
        .unwrap();
        let cmd = log_from_cli(cli);
        assert!(cmd.action.0.is_none());
        assert_eq!(cmd.limit, Some(0));
        assert_eq!(cmd.query, ["new", "second"]);
        assert_eq!(cmd.fuzzy, ["new"]);
    }

    #[test]
    fn search_updates_retain_rotated_and_accept_explicit_default_output() {
        let mut cli =
            crate::Cli::try_parse_from(["gwi", "log", "--rotated", "--output", "json"]).unwrap();
        cli.try_update_from(["gwi", "log", "--output", "oneline", "--follow"])
            .unwrap();
        let cmd = log_from_cli(cli);
        assert!(cmd.rotated);
        assert!(cmd.follow);
        assert_eq!(cmd.output, Format::Oneline);
    }

    #[test]
    fn action_updates_select_prune_and_retain_it_when_omitted() {
        let mut cli = crate::Cli::try_parse_from(["gwi", "log"]).unwrap();
        cli.try_update_from(["gwi", "log", "prune", "--older-than", "7d"])
            .unwrap();
        assert!(matches!(
            log_from_cli_ref(&cli).action.0,
            Some(LogAction::Prune(_))
        ));
        cli.try_update_from(["gwi", "log", "prune", "--dry-run"])
            .unwrap();
        cli.try_update_from(["gwi", "log"]).unwrap();
        assert!(matches!(
            log_from_cli(cli).action.0,
            Some(LogAction::Prune(_))
        ));
    }

    fn log_from_cli_ref(cli: &crate::Cli) -> &LogCommand {
        let crate::cli::Commands::Log(cmd) = &cli.command else {
            panic!("expected log"); // patchcov: coverage ignore-line reason="guards this test helper; every caller passes a parsed log argv"
        };
        cmd
    }

    #[test]
    fn updates_preserve_search_prune_input_conflicts() {
        for args in [
            vec!["gwi", "log", "--limit", "1", "prune"],
            vec!["gwi", "log", "--audit", "prune"],
            vec!["gwi", "log", "--query", "status:5xx", "prune"],
            vec!["gwi", "log", "prune", "--limit", "1"],
            vec!["gwi", "log", "--rotated", "--audit"],
        ] {
            let mut cli = crate::Cli::try_parse_from(["gwi", "log", "--limit", "0"]).unwrap();
            assert!(crate::Cli::try_parse_from(&args).is_err(), "{args:?}");
            assert!(cli.try_update_from(&args).is_err(), "{args:?}");
            let cmd = log_from_cli(cli);
            assert_eq!(cmd.limit, Some(0));
            assert!(cmd.action.0.is_none());
        }
    }

    #[test]
    fn optional_action_adapter_supports_immutable_and_mutable_matches() {
        use clap::{CommandFactory, FromArgMatches};

        let matches = LogCommand::command()
            .try_get_matches_from(["log", "prune", "--dry-run"])
            .unwrap();
        assert!(matches!(
            LogActions::from_arg_matches(&matches).unwrap().0,
            Some(LogAction::Prune(_))
        ));
        let mut action = LogActions(None);
        action.update_from_arg_matches(&matches).unwrap();
        assert!(matches!(action.0, Some(LogAction::Prune(_))));
        let empty = LogCommand::command_for_update()
            .try_get_matches_from(["log"])
            .unwrap();
        action.update_from_arg_matches(&empty).unwrap();
        assert!(matches!(action.0, Some(LogAction::Prune(_))));
        assert!(LogActions::from_arg_matches(&empty).unwrap().0.is_none());
    }

    #[test]
    fn defaults_are_sane() {
        let cmd = parse(&[]);
        assert_eq!(cmd.output, Format::Oneline);
        assert!(cmd.format.is_none());
        assert!(cmd.limit.is_none());
        assert!(!cmd.follow);
        assert!(!cmd.audit);
        assert!(!cmd.rotated);
    }

    #[test]
    fn rotated_flag_parses_and_conflicts_with_audit() {
        assert!(parse(&["--rotated"]).rotated);
        assert!(Wrapper::try_parse_from(["gwi", "log", "--rotated", "--audit"]).is_err());
    }

    #[test]
    fn audit_flag_parses() {
        assert!(parse(&["--audit"]).audit);
    }

    #[test]
    fn resolve_path_reads_audit_jsonl_only_when_the_flag_is_set() {
        let env = MapEnv::new().with("GWI_AUDIT_LOG_FILE", "/tmp/gwi-test-audit.jsonl");

        // Without `--audit`, this resolves through `log_file_path` — some
        // path other than the audit override (`GWI_LOG_FILE` is unset in
        // `env`, so it falls to the default: the state/data-dir path in a
        // release build, a scratch file in a test build). With `--audit`,
        // it must be exactly the override above.
        assert_ne!(
            parse(&[]).resolve_path_with(&env).unwrap(),
            std::path::PathBuf::from("/tmp/gwi-test-audit.jsonl")
        );
        assert_eq!(
            parse(&["--audit"]).resolve_path_with(&env).unwrap(),
            std::path::PathBuf::from("/tmp/gwi-test-audit.jsonl")
        );
    }

    #[test]
    fn search_flags_before_a_subcommand_are_refused_not_silently_dropped() {
        // Including `--audit`, which would otherwise look like it scoped the prune.
        for flag in [
            &["--since", "1d"][..],
            &["--query", "status:5xx"],
            &["--audit"],
            &["--rotated"],
        ] {
            let mut args = vec!["gwi", "log"];
            args.extend_from_slice(flag);
            args.extend_from_slice(&["prune", "--older-than", "7d"]);
            assert!(Wrapper::try_parse_from(args).is_err(), "{flag:?}");
        }
        // The subcommand's own flags, and the global `--profile`, still parse.
        let args = [
            "gwi",
            "--profile",
            "work",
            "log",
            "prune",
            "--older-than",
            "7d",
        ];
        assert!(crate::Cli::try_parse_from(args).is_ok());
    }

    #[test]
    fn deprecated_format_alias_still_parses() {
        let cmd = parse(&["--format", "json"]);
        // Captured separately; `execute` folds it into `output` with a warning.
        assert_eq!(cmd.format, Some(Format::Json));
        assert_eq!(cmd.output, Format::Oneline);
    }

    #[test]
    fn parses_full_flag_matrix() {
        let cmd = parse(&[
            "--since",
            "2h",
            "--method",
            "GET",
            "--status",
            "5xx",
            "--service",
            "gmail",
            "--command",
            "gmail read",
            "--url",
            "issue",
            "--grep",
            "X-\\d+",
            "--fuzzy",
            "a",
            "--fuzzy",
            "b",
            "--query",
            "status:5xx OR method:POST",
            "--id",
            "abc",
            "-o",
            "json",
            "-n",
            "10",
            "-f",
        ]);
        assert_eq!(cmd.since.as_deref(), Some("2h"));
        assert_eq!(cmd.status.as_deref(), Some("5xx"));
        assert_eq!(cmd.fuzzy, vec!["a", "b"]);
        assert_eq!(cmd.query.len(), 1);
        assert_eq!(cmd.output, Format::Json);
        assert_eq!(cmd.limit, Some(10));
        assert!(cmd.follow);
    }
}

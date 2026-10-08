//! `gwi log` — search and pretty-print the local invocation, HTTP and Drive-mutation log.
//!
//! Read-only and synchronous. Streams [`request_log::log_file_path`] line by
//! line, applies the filter matrix, and renders each match as `oneline`,
//! `json` (byte-identical to the on-disk NDJSON), or `full`.

mod format;
mod prune;
mod query;
mod stream;

use anyhow::{bail, Context, Result};
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
    Json,
    /// A labelled, multi-line block per record.
    Full,
}

/// Searches and pretty-prints the local invocation + HTTP request log.
///
/// With no subcommand, the flags below search the log; the `prune` subcommand
/// trims the log to bound its on-disk growth.
#[derive(Parser)]
pub struct LogCommand {
    /// Subcommand; when absent, the flags below search the log.
    #[command(subcommand)]
    action: Option<LogAction>,
    /// Lower time bound: a relative window (`30m`, `2h`, `1d`), a date
    /// (`2026-07-01`), or an RFC3339 timestamp.
    #[arg(long, value_name = "DUR_OR_TS")]
    since: Option<String>,
    /// Upper time bound: same forms as `--since` (a relative value means that
    /// long ago). Pair with `--since` for a bounded window.
    #[arg(long, value_name = "DUR_OR_TS")]
    until: Option<String>,
    /// Match the HTTP method (case-insensitive), e.g. `GET`.
    #[arg(long, value_name = "METHOD")]
    method: Option<String>,
    /// Match the status: exact (`200`), class (`5xx`), or list (`4xx,5xx`).
    #[arg(long, value_name = "STATUS")]
    status: Option<String>,
    /// Match the service tag, e.g. `gmail`, `drive`.
    #[arg(long, value_name = "NAME")]
    service: Option<String>,
    /// Match the resolved command path prefix, e.g. `"gmail search"`.
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
    /// AND-ed together.
    #[arg(long, value_name = "EXPR")]
    query: Vec<String>,
    /// Match this record `id` or `invocation_id` (pulls a run and its requests).
    #[arg(long, value_name = "ID")]
    id: Option<String>,
    /// Output format.
    #[arg(short = 'o', long, value_enum, default_value_t = Format::Oneline)]
    output: Format,
    /// Deprecated: use `-o`/`--output` instead.
    #[arg(long = "format", hide = true)]
    format: Option<Format>,
    /// Show at most N (most recent) matching records.
    #[arg(short = 'n', long, value_name = "N")]
    limit: Option<usize>,
    /// Follow the log, printing new matching records as they are appended.
    #[arg(short = 'f', long)]
    follow: bool,
    /// Read the fail-closed audit log (`audit.jsonl`) instead of the
    /// best-effort request log — the leased-write lifecycle and refusal
    /// trail (ADR-0080 §11), not subject to
    /// `GWI_LOG_DISABLE`, rotation or `prune`. Every filter, the
    /// `--query` mini-language and all three output formats apply
    /// unchanged; only the file being read differs.
    #[arg(long)]
    audit: bool,
}

/// A `log` subcommand. Absent = search (the flags on [`LogCommand`]).
#[derive(Subcommand)]
enum LogAction {
    /// Prune old records to bound the log's on-disk growth.
    Prune(prune::PruneCommand),
}

impl LogCommand {
    /// Executes the `gwi log` command.
    pub fn execute(mut self) -> Result<()> {
        if let Some(action) = self.action {
            // `self.audit` only affects the bare search below — a subcommand
            // never sees it, so silently proceeding here would make `--audit`
            // placed before the subcommand name a silent no-op instead of the
            // explicit refusal ADR-0080 §11 calls for. `prune` has its own
            // `--audit` (placed *after* the subcommand) that refuses loudly.
            if self.audit {
                bail!(
                    "--audit has no effect here: it must follow the subcommand, e.g. \
                     `gwi log prune --audit`, not `gwi log --audit prune`"
                );
            }
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
        stream::run(&path, &filter, self.output, self.limit, self.follow)
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
    fn defaults_are_sane() {
        let cmd = parse(&[]);
        assert_eq!(cmd.output, Format::Oneline);
        assert!(cmd.format.is_none());
        assert!(cmd.limit.is_none());
        assert!(!cmd.follow);
        assert!(!cmd.audit);
    }

    #[test]
    fn audit_flag_parses() {
        assert!(parse(&["--audit"]).audit);
    }

    #[test]
    fn resolve_path_reads_audit_jsonl_only_when_the_flag_is_set() {
        let env = MapEnv::new().with("GWI_AUDIT_LOG_FILE", "/tmp/gwi-test-audit.jsonl");

        // Without `--audit`, this resolves through `log_file_path` — some
        // path other than the audit override (`GWI_LOG_FILE` is unset
        // in `env`, so it falls to the state/data-dir default). With
        // `--audit`, it must be exactly the override above.
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
    fn top_level_audit_before_a_subcommand_is_refused_not_silently_dropped() {
        // `--audit` only wires into the bare-search path; placed before a
        // subcommand it must never be a silent no-op (it used to be, since
        // `execute` returned out of the subcommand match before checking
        // `self.audit` at all).
        let err = parse(&["--audit", "prune", "--older-than", "7d"])
            .execute()
            .unwrap_err();
        assert!(format!("{err}").contains("gwi log prune --audit"), "{err}");
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
            "gmail search",
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

//! Filter construction and the `--query` mini-language.
//!
//! A [`Filter`] is the AND of every supplied flag plus every `--query`
//! expression. The query language supports `AND`/`OR`/`NOT` (and a leading `-`
//! for negation), parentheses, `field:value` structured terms, and bare fuzzy
//! tokens matched against the raw JSON line. A word that is exactly one quoted
//! string (`"not"`, `"12:34:56"`) is always a bare token: never an operator and
//! never a `field:value` term. Status matching is shared with the structured
//! flags so `--status 5xx` and `status:5xx` (and `--status blocked` and
//! `status:blocked`) behave identically.
//!
//! A `field:value` term whose field is not a built-in name falls back to the
//! record's free-form `context` map. That fallback cannot tell a typo from a
//! real context key, so the [`Filter`] watches which context keys it sees and
//! [`Filter::unknown_field_warnings`] reports the ones that matched nothing.
//! Likewise a domain status word (`--status blokced`, `status:blokced`) is
//! free-form text that cannot be checked up front, so the [`Filter`] watches the
//! statuses of the `drivemutation` records it sees and
//! [`Filter::unseen_status_warnings`] reports the words that no record has.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use regex::Regex;

use crate::request_log::{LogRecord, RecordKind, Source};
use crate::utils::duration::parse_since;

/// The raw, borrowed flag values used to build a [`Filter`].
pub struct FilterInput<'a> {
    pub since: Option<&'a str>,
    pub until: Option<&'a str>,
    pub method: Option<&'a str>,
    pub status: Option<&'a str>,
    pub service: Option<&'a str>,
    pub command: Option<&'a str>,
    pub url: Option<&'a str>,
    pub grep: Option<&'a str>,
    pub fuzzy: &'a [String],
    pub query: &'a [String],
    pub id: Option<&'a str>,
}

/// A compiled predicate over [`LogRecord`] lines.
pub struct Filter {
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    method: Option<String>,
    status: Option<StatusFilter>,
    service: Option<String>,
    command: Option<String>,
    url: Option<String>,
    grep: Option<Regex>,
    fuzzy: Vec<String>,
    id: Option<String>,
    queries: Vec<Expr>,
    watch: RefCell<FieldWatch>,
    status_watch: RefCell<StatusWatch>,
}

impl Filter {
    /// Compiles all flags and query expressions up front, surfacing parse
    /// errors (bad regex/status/duration/query) before streaming begins.
    pub fn build(input: FilterInput<'_>) -> Result<Self> {
        let since = match input.since {
            Some(s) => Some(parse_time_bound(s)?),
            None => None,
        };
        let until = match input.until {
            Some(s) => Some(parse_time_bound(s)?),
            None => None,
        };
        let status = match input.status {
            Some(s) => Some(StatusFilter::parse(s)?),
            None => None,
        };
        let grep = match input.grep {
            Some(s) => Some(Regex::new(s).with_context(|| format!("invalid --grep regex: {s}"))?),
            None => None,
        };
        let mut queries = Vec::new();
        for q in input.query {
            queries.push(parse_query(q).with_context(|| format!("invalid --query: {q}"))?);
        }
        let watch = FieldWatch::new(&queries);
        let status_watch = StatusWatch::new(status.as_ref(), &queries);
        Ok(Self {
            since,
            until,
            method: input.method.map(str::to_string),
            status,
            service: input.service.map(str::to_string),
            command: input.command.map(str::to_string),
            url: input.url.map(str::to_string),
            grep,
            fuzzy: input.fuzzy.to_vec(),
            id: input.id.map(str::to_string),
            queries,
            watch: RefCell::new(watch),
            status_watch: RefCell::new(status_watch),
        })
    }

    /// One warning per `field:value` query term whose field is not a built-in
    /// name and appeared in the `context` of none of the records scanned, with a
    /// suggestion when it is a near miss. Empty when nothing was scanned (there
    /// is nothing to judge against) or every such field was seen. With
    /// `following`, the log is still growing, so the warning says the field
    /// matched nothing *so far* rather than claiming it never will.
    pub fn unknown_field_warnings(&self, following: bool) -> Vec<String> {
        self.watch.borrow().warnings(following)
    }

    /// One warning per domain status word (`blocked`, from `--status` or a
    /// `status:` term) that is the `context.status` of none of the
    /// `drivemutation` records scanned, with a suggestion when it is a near miss
    /// of a status that was seen. Empty when no `drivemutation` record was
    /// scanned (there is nothing to judge against) or every word was seen. With
    /// `following`, the warning says the word matched nothing *so far*.
    pub fn unseen_status_warnings(&self, following: bool) -> Vec<String> {
        self.status_watch.borrow().warnings(following)
    }

    /// Whether `rec` (whose verbatim JSON line is `raw`) passes every clause.
    pub fn matches(&self, rec: &LogRecord, raw: &str) -> bool {
        self.watch.borrow_mut().observe(rec);
        self.status_watch.borrow_mut().observe(rec);

        // Only the fuzzy and query clauses read the lowercased line, so skip the
        // copy for the common structured-flag-only search.
        let raw_lower = if self.fuzzy.is_empty() && self.queries.is_empty() {
            String::new()
        } else {
            raw.to_ascii_lowercase()
        };

        if let Some(cutoff) = self.since {
            match parse_timestamp(&rec.timestamp) {
                Some(ts) if ts >= cutoff => {}
                _ => return false,
            }
        }
        if let Some(cutoff) = self.until {
            match parse_timestamp(&rec.timestamp) {
                Some(ts) if ts <= cutoff => {}
                _ => return false,
            }
        }
        if let Some(m) = &self.method {
            if !opt_eq_ci(rec.method.as_deref(), m) {
                return false;
            }
        }
        if let Some(s) = &self.status {
            if !s.matches(rec) {
                return false;
            }
        }
        if let Some(s) = &self.service {
            if !opt_eq_ci(rec.service.as_deref(), s) {
                return false;
            }
        }
        if let Some(c) = &self.command {
            if !command_matches(rec, c) {
                return false;
            }
        }
        if let Some(u) = &self.url {
            if !contains_ci(rec.url.as_deref(), u) {
                return false;
            }
        }
        if let Some(re) = &self.grep {
            if !re.is_match(raw) {
                return false;
            }
        }
        for token in &self.fuzzy {
            if !raw_lower.contains(&token.to_ascii_lowercase()) {
                return false;
            }
        }
        if let Some(id) = &self.id {
            if &rec.id != id && &rec.invocation_id != id {
                return false;
            }
        }
        for q in &self.queries {
            if !q.eval(rec, &raw_lower) {
                return false;
            }
        }
        true
    }
}

/// Parses a `--since`/`--until` bound, trying three forms in order:
///
/// 1. An absolute RFC3339 timestamp (`2026-07-01T12:00:00Z`).
/// 2. A date-only `YYYY-MM-DD`, interpreted as midnight UTC.
/// 3. A relative duration back from now (delegated to [`parse_since`]).
///
/// Relative durations never parse as RFC3339 or a date, so the ordering is
/// unambiguous. An absolute value is that instant; a relative value is
/// "now minus the duration".
pub(crate) fn parse_time_bound(s: &str) -> Result<DateTime<Utc>> {
    let s = s.trim();

    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(date.and_time(chrono::NaiveTime::MIN).and_utc());
    }

    parse_since(s)
}

/// Parses an RFC3339 timestamp into UTC, or `None` if absent/unparseable.
fn parse_timestamp(ts: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// A status filter: a set of exact codes and/or `Nxx` classes.
struct StatusMatcher {
    exact: Vec<u16>,
    classes: Vec<u8>,
}

impl StatusMatcher {
    /// Parses `"200"`, `"5xx"`, or a comma list like `"4xx,5xx"`.
    fn parse(spec: &str) -> Result<Self> {
        let mut exact = Vec::new();
        let mut classes = Vec::new();
        for part in spec.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let lower = part.to_ascii_lowercase();
            if let Some(first) = lower.strip_suffix("xx") {
                let digit: u8 = first
                    .parse()
                    .with_context(|| format!("invalid status class: {part}"))?;
                if !(1..=5).contains(&digit) {
                    bail!("invalid status class: {part}");
                }
                classes.push(digit);
            } else {
                exact.push(
                    part.parse()
                        .with_context(|| format!("invalid status code: {part}"))?,
                );
            }
        }
        if exact.is_empty() && classes.is_empty() {
            bail!("empty --status filter");
        }
        Ok(Self { exact, classes })
    }

    /// Whether a (possibly absent) status code matches this filter.
    fn matches(&self, code: Option<u16>) -> bool {
        let Some(code) = code else {
            return false;
        };
        self.exact.contains(&code) || self.classes.contains(&((code / 100) as u8))
    }
}

/// A compiled `--status` filter, accepting every form `status:` does.
enum StatusFilter {
    /// Exact codes and `Nxx` classes (`200`, `5xx`, `4xx,5xx`).
    Codes(StatusMatcher),
    /// A numeric comparison (`>=400`), kept as the spec for [`numeric_match`].
    Compare(String),
    /// Drive-mutation domain statuses (`blocked`, `blocked,written`).
    Domain(String),
}

impl StatusFilter {
    /// Parses a `--status` value. A leading comparator selects a numeric
    /// comparison, a leading letter selects the domain statuses of a
    /// `drivemutation` record, and anything else is codes and classes, which
    /// are validated strictly so `9xx` or `4xx,blocked` still fail up front.
    fn parse(spec: &str) -> Result<Self> {
        let spec = spec.trim();
        if has_comparator(spec) {
            split_comparator(spec)
                .0
                .trim()
                .parse::<i64>()
                .with_context(|| format!("invalid status comparison: {spec}"))?;
            Ok(Self::Compare(spec.to_string()))
        } else if spec.starts_with(|c: char| c.is_ascii_alphabetic()) {
            Ok(Self::Domain(spec.to_string()))
        } else {
            StatusMatcher::parse(spec).map(Self::Codes)
        }
    }

    /// The domain status words this filter looks for, if it is one.
    fn domain_words(&self) -> Vec<&str> {
        match self {
            Self::Domain(spec) => domain_status_words(spec).collect(),
            Self::Codes(_) | Self::Compare(_) => Vec::new(),
        }
    }

    /// Whether `rec` passes: the same answer `status:<spec>` gives.
    fn matches(&self, rec: &LogRecord) -> bool {
        match self {
            Self::Codes(m) => m.matches(rec.status_code),
            Self::Compare(spec) => numeric_match(rec.status_code.map(i64::from), spec),
            Self::Domain(spec) => domain_status_matches(rec, spec),
        }
    }
}

/// The trimmed, non-empty words of a comma-separated domain status spec.
fn status_words(spec: &str) -> impl Iterator<Item = &str> {
    spec.split(',').map(str::trim).filter(|w| !w.is_empty())
}

/// The words of a domain status spec that name a status (start with a letter),
/// so a stray code or class in a mixed list (`blocked,5xx`) is not mistaken for
/// a mistyped status.
fn domain_status_words(spec: &str) -> impl Iterator<Item = &str> {
    status_words(spec).filter(|w| w.starts_with(|c: char| c.is_ascii_alphabetic()))
}

/// Whether a `drivemutation` record's domain status (`context["status"]`) is
/// one of the comma-separated `spec` values. Exact, not substring: the status
/// set is enum-like and several values share substrings (`written` /
/// `would-write`, `blocked` / `refused-*`). Other kinds never match.
fn domain_status_matches(rec: &LogRecord, spec: &str) -> bool {
    rec.kind == RecordKind::DriveMutation
        && rec
            .context
            .get("status")
            .is_some_and(|status| status_words(spec).any(|want| status.eq_ignore_ascii_case(want)))
}

/// Whether the record's command path matches `prefix` on whole path segments
/// (so `gmail` matches `["gmail","read"]` but `jir` does not).
fn command_matches(rec: &LogRecord, prefix: &str) -> bool {
    let joined = rec.command.join(" ");
    let prefix = prefix.trim();
    joined == prefix || joined.starts_with(&format!("{prefix} "))
}

/// Case-insensitive equality against an optional field.
fn opt_eq_ci(field: Option<&str>, value: &str) -> bool {
    field.is_some_and(|f| f.eq_ignore_ascii_case(value))
}

/// Case-insensitive substring against an optional field.
fn contains_ci(field: Option<&str>, value: &str) -> bool {
    field.is_some_and(|f| f.to_ascii_lowercase().contains(&value.to_ascii_lowercase()))
}

/// Lowercase string form of a [`Source`].
fn source_str(source: Source) -> &'static str {
    match source {
        Source::Cli => "cli",
        Source::Mcp => "mcp",
        Source::Unknown => "unknown",
    }
}

/// The built-in field names and aliases [`builtin_field_matches`] handles. Used
/// only to suggest a correction; a test keeps it in step with the match arms.
const BUILTIN_FIELDS: &[&str] = &[
    "kind",
    "source",
    "service",
    "method",
    "status",
    "command",
    "cmd",
    "url",
    "id",
    "invocation_id",
    "inv",
    "mcp_tool",
    "tool",
    "error",
    "err",
    "exit_code",
    "exit",
    "duration_ms",
    "duration",
    "dur",
    "elapsed_ms",
    "elapsed",
    "hostname",
    "host",
    "system_user",
    "user",
    "cwd",
    "auth_principal",
    "principal",
];

/// Evaluates a `field:value` term against a record (shared by the query AST).
fn field_matches(rec: &LogRecord, field: &str, value: &str) -> bool {
    builtin_field_matches(rec, field, value).unwrap_or_else(|| {
        // Unknown field names fall back to the free-form `context` map
        // (HTTP correlation tags and the Drive-mutation and audit fields such
        // as `file_id`, `lease_id` and `verdict`): case-insensitive substring, like `url`.
        contains_ci(
            rec.context
                .get(&field.to_ascii_lowercase())
                .map(String::as_str),
            value,
        )
    })
}

/// Evaluates a `field:value` term against a built-in field, or `None` when
/// `field` is not one (so the caller falls back to the `context` map).
fn builtin_field_matches(rec: &LogRecord, field: &str, value: &str) -> Option<bool> {
    Some(match field.to_ascii_lowercase().as_str() {
        "kind" => rec.kind.as_str().eq_ignore_ascii_case(value),
        "source" => rec
            .source
            .is_some_and(|s| source_str(s).eq_ignore_ascii_case(value)),
        "service" => opt_eq_ci(rec.service.as_deref(), value),
        "method" => opt_eq_ci(rec.method.as_deref(), value),
        // A `drivemutation` record has no `status_code` — its domain status
        // (`blocked`, `written`, `stale-revision`, ...) lives in
        // `context["status"]`, matched exactly (see [`domain_status_matches`]).
        "status" if rec.kind == RecordKind::DriveMutation => domain_status_matches(rec, value),
        // `status` keeps its class syntax (`5xx`, `4xx,5xx`); a leading
        // comparator (`status:>=400`) routes to the numeric matcher instead.
        "status" if has_comparator(value) => numeric_match(rec.status_code.map(i64::from), value),
        "status" => StatusMatcher::parse(value).is_ok_and(|m| m.matches(rec.status_code)),
        "command" | "cmd" => command_matches(rec, value),
        "url" => contains_ci(rec.url.as_deref(), value),
        "id" => rec.id == value || rec.invocation_id == value,
        "invocation_id" | "inv" => rec.invocation_id == value,
        "mcp_tool" | "tool" => opt_eq_ci(rec.mcp_tool.as_deref(), value),
        "error" | "err" => match rec.error.as_deref() {
            Some(e) if value.is_empty() || value == "true" => !e.is_empty() || value == "true",
            Some(e) => e.to_ascii_lowercase().contains(&value.to_ascii_lowercase()),
            None => false,
        },
        // Numeric fields: `>N`, `>=N`, `<N`, `<=N`, or bare `N` (equality).
        "exit_code" | "exit" => numeric_match(rec.exit_code.map(i64::from), value),
        "duration_ms" | "duration" | "dur" => {
            numeric_match(rec.duration_ms.map(|v| v as i64), value)
        }
        "elapsed_ms" | "elapsed" => numeric_match(rec.elapsed_ms.map(|v| v as i64), value),
        // Text fields: case-insensitive substring, like `url`.
        "hostname" | "host" => contains_ci(Some(&rec.hostname), value),
        "system_user" | "user" => contains_ci(Some(&rec.system_user), value),
        "cwd" => contains_ci(Some(&rec.cwd), value),
        "auth_principal" | "principal" => contains_ci(rec.auth_principal.as_deref(), value),
        _ => return None,
    })
}

/// Whether `field` is a built-in name rather than a `context` key.
fn is_builtin_field(field: &str) -> bool {
    builtin_field_matches(&LogRecord::default(), field, "").is_some()
}

/// Whether a numeric-field value carries a leading comparison operator
/// (`>`, `<`, `=`) rather than a bare number or a status class.
fn has_comparator(spec: &str) -> bool {
    spec.trim_start().starts_with(['>', '<', '='])
}

/// Splits a numeric spec into its operand and comparison: `>N`, `>=N`, `<N`,
/// `<=N`, `=N`, or a bare `N` (equality).
fn split_comparator(spec: &str) -> (&str, fn(i64, i64) -> bool) {
    let spec = spec.trim();
    if let Some(r) = spec.strip_prefix(">=") {
        (r, |a, b| a >= b)
    } else if let Some(r) = spec.strip_prefix("<=") {
        (r, |a, b| a <= b)
    } else if let Some(r) = spec.strip_prefix('>') {
        (r, |a, b| a > b)
    } else if let Some(r) = spec.strip_prefix('<') {
        (r, |a, b| a < b)
    } else if let Some(r) = spec.strip_prefix('=') {
        (r, |a, b| a == b)
    } else {
        (spec, |a, b| a == b)
    }
}

/// Matches a numeric record field against a spec that may carry a leading
/// comparator: `>N`, `>=N`, `<N`, `<=N`, `=N`, or a bare `N` (equality). An
/// absent field never matches; an unparseable number never matches.
fn numeric_match(field: Option<i64>, spec: &str) -> bool {
    let Some(actual) = field else {
        return false;
    };
    let (rest, cmp) = split_comparator(spec);
    match rest.trim().parse::<i64>() {
        Ok(n) => cmp(actual, n),
        Err(_) => false,
    }
}

// --- `--query` mini-language ---

/// A parsed query expression tree.
enum Expr {
    And(Box<Self>, Box<Self>),
    Or(Box<Self>, Box<Self>),
    Not(Box<Self>),
    /// `field:value` structured term.
    Field(String, String),
    /// Bare fuzzy token, matched against the lowercased raw line.
    Term(String),
}

impl Expr {
    /// Evaluates against a record and its lowercased raw JSON line.
    fn eval(&self, rec: &LogRecord, raw_lower: &str) -> bool {
        match self {
            Self::And(a, b) => a.eval(rec, raw_lower) && b.eval(rec, raw_lower),
            Self::Or(a, b) => a.eval(rec, raw_lower) || b.eval(rec, raw_lower),
            Self::Not(a) => !a.eval(rec, raw_lower),
            Self::Field(f, v) => field_matches(rec, f, v),
            Self::Term(t) => raw_lower.contains(&t.to_ascii_lowercase()),
        }
    }

    /// Appends every `field:value` term whose field is not a built-in name
    /// (so it is looked up in the `context` map) as `(field, value)`.
    fn collect_context_terms<'a>(&'a self, out: &mut Vec<(&'a str, &'a str)>) {
        match self {
            Self::And(a, b) | Self::Or(a, b) => {
                a.collect_context_terms(out);
                b.collect_context_terms(out);
            }
            Self::Not(a) => a.collect_context_terms(out),
            Self::Field(f, v) if !is_builtin_field(f) => out.push((f, v)),
            Self::Field(..) | Self::Term(_) => {}
        }
    }

    /// Appends every domain status word of a `status:` term as `(word, term)`.
    /// A value that does not start with a letter is a code, class or comparison
    /// (`5xx`, `>=400`), not a domain status, as for `--status`.
    fn collect_status_terms<'a>(&'a self, out: &mut Vec<(&'a str, String)>) {
        match self {
            Self::And(a, b) | Self::Or(a, b) => {
                a.collect_status_terms(out);
                b.collect_status_terms(out);
            }
            Self::Not(a) => a.collect_status_terms(out),
            Self::Field(f, v)
                if f.eq_ignore_ascii_case("status")
                    && v.trim_start()
                        .starts_with(|c: char| c.is_ascii_alphabetic()) =>
            {
                out.extend(domain_status_words(v).map(|w| (w, format!("status:{w}"))));
            }
            Self::Field(..) | Self::Term(_) => {}
        }
    }
}

/// Tracks, while records are scanned, whether each `context`-fallback query
/// field ever appears as a `context` key, so a typo'd field name (which
/// otherwise matches nothing without a word) can be reported afterwards.
struct FieldWatch {
    /// Fields not yet seen in any record's `context`, keyed by lowercased name,
    /// with the first `field:value` term that used each, for the message.
    unseen: BTreeMap<String, String>,
    /// Every `context` key seen so far; the pool for "did you mean". Only
    /// collected while a field is unseen, which is the only time it is needed.
    keys: BTreeSet<String>,
    /// Records scanned (parsed lines presented to the filter).
    scanned: u64,
}

impl FieldWatch {
    fn new(queries: &[Expr]) -> Self {
        let mut terms = Vec::new();
        for q in queries {
            q.collect_context_terms(&mut terms);
        }
        let mut unseen = BTreeMap::new();
        for (field, value) in terms {
            unseen
                .entry(field.to_ascii_lowercase())
                .or_insert_with(|| format!("{field}:{value}"));
        }
        Self {
            unseen,
            keys: BTreeSet::new(),
            scanned: 0,
        }
    }

    fn observe(&mut self, rec: &LogRecord) {
        if self.unseen.is_empty() {
            return;
        }
        self.scanned += 1;
        for key in rec.context.keys() {
            self.unseen.remove(key);
            if !self.keys.contains(key) {
                self.keys.insert(key.clone());
            }
        }
    }

    fn warnings(&self, following: bool) -> Vec<String> {
        if self.scanned == 0 {
            return Vec::new();
        }
        let scanned = if self.scanned == 1 {
            "the 1 record scanned".to_string()
        } else {
            format!("any of the {} records scanned", self.scanned)
        };
        let so_far = if following { " so far" } else { "" };
        self.unseen
            .iter()
            .map(|(field, term)| {
                let hint = match suggest(field, &self.keys) {
                    Some(name) => format!(" Did you mean `{name}`?"),
                    None => String::new(),
                };
                format!(
                    "warning: query field `{field}` is not a built-in field and is not a context key \
                     in {scanned}, so `{term}` matches nothing{so_far}.{hint} \
                     To search for the text instead, quote it: \"{term}\""
                )
            })
            .collect()
    }
}

/// Tracks, while records are scanned, which domain statuses the `drivemutation`
/// records carry, so a typo'd `--status` / `status:` word (which otherwise
/// matches nothing without a word) can be reported afterwards. The statuses are
/// free-form text, so a word is only judged against what was actually seen.
struct StatusWatch {
    /// Words not yet seen as any record's status, keyed by lowercased word,
    /// with the flag or term that used each first, for the message.
    unseen: BTreeMap<String, String>,
    /// Every status seen so far (lowercased); the pool for "did you mean".
    seen: BTreeSet<String>,
    /// `drivemutation` records scanned.
    mutations: u64,
}

impl StatusWatch {
    fn new(status: Option<&StatusFilter>, queries: &[Expr]) -> Self {
        let mut unseen = BTreeMap::new();
        let flag = status.map(StatusFilter::domain_words).unwrap_or_default();
        for word in flag {
            unseen
                .entry(word.to_ascii_lowercase())
                .or_insert_with(|| format!("--status {word}"));
        }
        let mut terms = Vec::new();
        for q in queries {
            q.collect_status_terms(&mut terms);
        }
        for (word, term) in terms {
            unseen.entry(word.to_ascii_lowercase()).or_insert(term);
        }
        Self {
            unseen,
            seen: BTreeSet::new(),
            mutations: 0,
        }
    }

    fn observe(&mut self, rec: &LogRecord) {
        if self.unseen.is_empty() || rec.kind != RecordKind::DriveMutation {
            return;
        }
        self.mutations += 1;
        if let Some(status) = rec.context.get("status") {
            // Case-insensitive scans of two tiny sets, so no per-record allocation.
            self.unseen
                .retain(|word, _| !word.eq_ignore_ascii_case(status));
            if !self.seen.iter().any(|s| s.eq_ignore_ascii_case(status)) {
                self.seen.insert(status.to_ascii_lowercase());
            }
        }
    }

    fn warnings(&self, following: bool) -> Vec<String> {
        if self.mutations == 0 {
            return Vec::new();
        }
        let scanned = if self.mutations == 1 {
            "the 1 drivemutation record scanned".to_string()
        } else {
            format!(
                "any of the {} drivemutation records scanned",
                self.mutations
            )
        };
        let so_far = if following { " so far" } else { "" };
        self.unseen
            .iter()
            .map(|(word, term)| {
                let hint = match nearest(word, self.seen.iter().map(String::as_str)) {
                    Some(status) => format!(" Did you mean `{status}`?"),
                    None => String::new(),
                };
                format!(
                    "warning: no status `{word}` in {scanned}, so `{term}` matches no record{so_far}.{hint}"
                )
            })
            .collect()
    }
}

/// The closest built-in field or seen `context` key to `field`, if one is within
/// a small edit distance (so `servce` suggests `service`).
fn suggest<'a>(field: &str, keys: &'a BTreeSet<String>) -> Option<&'a str> {
    nearest(
        field,
        BUILTIN_FIELDS
            .iter()
            .copied()
            .chain(keys.iter().map(String::as_str)),
    )
}

/// The candidate closest to `word`, if one is within a small edit distance and
/// not identical to it.
fn nearest<'a>(word: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let max = if word.len() <= 4 { 1 } else { 2 };
    candidates
        .map(|candidate| (edit_distance(word, candidate), candidate))
        .filter(|&(d, _)| (1..=max).contains(&d))
        .min_by_key(|&(d, _)| d)
        .map(|(_, candidate)| candidate)
}

/// Levenshtein distance between two strings, by character.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, &cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != cb);
            cur.push(sub.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// A query token stream cursor for the recursive-descent parser.
#[derive(Debug, PartialEq, Eq)]
enum Token {
    LParen,
    RParen,
    And,
    Or,
    Not,
    Word(String),
    /// A word that was exactly one quoted string: text, never an operator or a
    /// `field:value` term.
    Literal(String),
}

/// Splits a query string into tokens, honoring parentheses, `"quoted values"`,
/// and a leading `-` as negation. A word that is nothing but one quoted string
/// becomes a [`Token::Literal`], so `"not"` searches for the word `not`.
fn tokenize(input: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '(' => {
                chars.next();
                tokens.push(Token::LParen);
            }
            ')' => {
                chars.next();
                tokens.push(Token::RParen);
            }
            '-' => {
                chars.next();
                // A lone '-' is a stray; '-foo' negates the following word.
                tokens.push(Token::Not);
            }
            _ => {
                let mut word = String::new();
                let mut quoted = 0;
                let mut bare = false;
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || c == '(' || c == ')' {
                        break;
                    }
                    if c == '"' {
                        quoted += 1;
                        chars.next();
                        for qc in chars.by_ref() {
                            if qc == '"' {
                                break;
                            }
                            word.push(qc);
                        }
                        continue;
                    }
                    bare = true;
                    word.push(c);
                    chars.next();
                }
                if quoted == 1 && !bare {
                    if word.is_empty() {
                        bail!("empty quoted term in query");
                    }
                    tokens.push(Token::Literal(word));
                    continue;
                }
                match word.to_ascii_uppercase().as_str() {
                    "AND" => tokens.push(Token::And),
                    "OR" => tokens.push(Token::Or),
                    "NOT" => tokens.push(Token::Not),
                    _ => tokens.push(Token::Word(word)),
                }
            }
        }
    }
    Ok(tokens)
}

/// Parses a `--query` expression into an [`Expr`] tree.
fn parse_query(input: &str) -> Result<Expr> {
    let tokens = tokenize(input)?;
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.parse_or()?;
    if parser.pos != parser.tokens.len() {
        bail!("unexpected trailing tokens in query");
    }
    Ok(expr)
}

/// Recursive-descent parser: `or := and ("OR" and)*`,
/// `and := unary (("AND")? unary)*`, `unary := "NOT" unary | primary`,
/// `primary := "(" or ")" | term`.
struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn parse_or(&mut self) -> Result<Expr> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Some(Token::Or)) {
            self.pos += 1;
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr> {
        let mut left = self.parse_unary()?;
        loop {
            match self.peek() {
                Some(Token::And) => {
                    self.pos += 1;
                    let right = self.parse_unary()?;
                    left = Expr::And(Box::new(left), Box::new(right));
                }
                // Implicit AND between adjacent terms (stop at OR/`)`/EOF).
                Some(Token::Word(_) | Token::Literal(_) | Token::Not | Token::LParen) => {
                    let right = self.parse_unary()?;
                    left = Expr::And(Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr> {
        if matches!(self.peek(), Some(Token::Not)) {
            self.pos += 1;
            return Ok(Expr::Not(Box::new(self.parse_unary()?)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        match self.tokens.get(self.pos) {
            Some(Token::LParen) => {
                self.pos += 1;
                let inner = self.parse_or()?;
                match self.tokens.get(self.pos) {
                    Some(Token::RParen) => {
                        self.pos += 1;
                        Ok(inner)
                    }
                    _ => bail!("unbalanced parenthesis in query"),
                }
            }
            Some(Token::Word(word)) => {
                let word = word.clone();
                self.pos += 1;
                Ok(match word.split_once(':') {
                    Some((field, value)) if !field.is_empty() => {
                        Expr::Field(field.to_string(), value.to_string())
                    }
                    _ => Expr::Term(word),
                })
            }
            Some(Token::Literal(text)) => {
                let text = text.clone();
                self.pos += 1;
                Ok(Expr::Term(text))
            }
            _ => bail!("expected a term in query"),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::request_log::RecordKind;

    fn http(status: Option<u16>, service: &str, method: &str) -> LogRecord {
        LogRecord {
            kind: RecordKind::Http,
            service: Some(service.to_string()),
            method: Some(method.to_string()),
            status_code: status,
            ..LogRecord::default()
        }
    }

    #[test]
    fn status_matcher_handles_exact_class_and_list() {
        let m = StatusMatcher::parse("200").unwrap();
        assert!(m.matches(Some(200)));
        assert!(!m.matches(Some(201)));

        let m = StatusMatcher::parse("5xx").unwrap();
        assert!(m.matches(Some(503)));
        assert!(!m.matches(Some(404)));
        assert!(!m.matches(None));

        let m = StatusMatcher::parse("4xx,5xx").unwrap();
        assert!(m.matches(Some(404)));
        assert!(m.matches(Some(500)));
        assert!(!m.matches(Some(204)));
    }

    #[test]
    fn status_matcher_rejects_garbage() {
        assert!(StatusMatcher::parse("9xx").is_err());
        assert!(StatusMatcher::parse("abc").is_err());
        assert!(StatusMatcher::parse("").is_err());
    }

    #[test]
    fn since_parses_units() {
        assert!(parse_time_bound("30m").is_ok());
        assert!(parse_time_bound("2h").is_ok());
        assert!(parse_time_bound("1d").is_ok());
        assert!(parse_time_bound("1w").is_ok());
        assert!(parse_time_bound("10x").is_err());
        assert!(parse_time_bound("h").is_err());
    }

    #[test]
    fn command_prefix_matches() {
        let rec = LogRecord {
            command: vec!["gmail".to_string(), "read".to_string()],
            ..LogRecord::default()
        };
        assert!(command_matches(&rec, "gmail"));
        assert!(command_matches(&rec, "gmail read"));
        assert!(!command_matches(&rec, "git"));
    }

    #[test]
    fn query_field_and_implicit_and() {
        let rec = http(Some(500), "gmail", "POST");
        let expr = parse_query("status:5xx service:gmail").unwrap();
        assert!(expr.eval(&rec, "{}"));
        let expr = parse_query("status:5xx service:drive").unwrap();
        assert!(!expr.eval(&rec, "{}"));
    }

    #[test]
    fn query_or_not_and_parens() {
        let rec = http(Some(404), "gmail", "GET");
        assert!(parse_query("status:5xx OR status:4xx")
            .unwrap()
            .eval(&rec, "{}"));
        assert!(parse_query("NOT status:5xx").unwrap().eval(&rec, "{}"));
        assert!(parse_query("-status:5xx").unwrap().eval(&rec, "{}"));
        assert!(parse_query("(status:4xx OR status:5xx) method:GET")
            .unwrap()
            .eval(&rec, "{}"));
        assert!(!parse_query("(status:4xx OR status:5xx) method:POST")
            .unwrap()
            .eval(&rec, "{}"));
    }

    #[test]
    fn query_bare_token_is_fuzzy_on_raw() {
        let rec = LogRecord::default();
        let expr = parse_query("deploy").unwrap();
        assert!(expr.eval(&rec, r#"{"url":"/api/deploy"}"#));
        assert!(!expr.eval(&rec, r#"{"url":"/api/status"}"#));
    }

    #[test]
    fn query_rejects_unbalanced_parens() {
        assert!(parse_query("(status:5xx").is_err());
        assert!(parse_query("status:5xx)").is_err());
    }

    #[test]
    fn filter_ands_flags_together() {
        let rec = http(Some(500), "gmail", "GET");
        let pass = Filter::build(FilterInput {
            since: None,
            until: None,
            method: Some("GET"),
            status: Some("5xx"),
            service: Some("gmail"),
            command: None,
            url: None,
            grep: None,
            fuzzy: &[],
            query: &[],
            id: None,
        })
        .unwrap();
        assert!(pass.matches(&rec, "{}"));

        let fail = Filter::build(FilterInput {
            since: None,
            until: None,
            method: Some("POST"),
            status: None,
            service: None,
            command: None,
            url: None,
            grep: None,
            fuzzy: &[],
            query: &[],
            id: None,
        })
        .unwrap();
        assert!(!fail.matches(&rec, "{}"));
    }

    fn empty_input<'a>() -> FilterInput<'a> {
        FilterInput {
            since: None,
            until: None,
            method: None,
            status: None,
            service: None,
            command: None,
            url: None,
            grep: None,
            fuzzy: &[],
            query: &[],
            id: None,
        }
    }

    fn rec_http() -> LogRecord {
        LogRecord {
            id: "rec-1".to_string(),
            invocation_id: "inv-9".to_string(),
            kind: RecordKind::Http,
            timestamp: "2026-06-22T10:00:00.000Z".to_string(),
            service: Some("gmail".to_string()),
            method: Some("GET".to_string()),
            url: Some("https://gmail.googleapis.com/gmail/v1/users/me/messages/X-1".to_string()),
            status_code: Some(200),
            ..LogRecord::default()
        }
    }

    #[test]
    fn build_rejects_bad_inputs() {
        let mut i = empty_input();
        i.grep = Some("(");
        assert!(Filter::build(i).is_err(), "bad regex");

        let mut i = empty_input();
        i.status = Some("9xx");
        assert!(Filter::build(i).is_err(), "bad status class");

        let mut i = empty_input();
        i.since = Some("bogus");
        assert!(Filter::build(i).is_err(), "bad since");

        let bad_query = vec!["(unclosed".to_string()];
        let mut i = empty_input();
        i.query = &bad_query;
        assert!(Filter::build(i).is_err(), "bad query");
    }

    #[test]
    fn matches_each_flag() {
        let rec = rec_http();
        let raw = serde_json::to_string(&rec).unwrap();

        let mut i = empty_input();
        i.method = Some("get");
        assert!(Filter::build(i).unwrap().matches(&rec, &raw));
        let mut i = empty_input();
        i.method = Some("post");
        assert!(!Filter::build(i).unwrap().matches(&rec, &raw));

        let mut i = empty_input();
        i.service = Some("gmail");
        assert!(Filter::build(i).unwrap().matches(&rec, &raw));

        let mut i = empty_input();
        i.url = Some("messages/X-1");
        assert!(Filter::build(i).unwrap().matches(&rec, &raw));
        let mut i = empty_input();
        i.url = Some("nope");
        assert!(!Filter::build(i).unwrap().matches(&rec, &raw));

        let mut i = empty_input();
        i.grep = Some("X-\\d+");
        assert!(Filter::build(i).unwrap().matches(&rec, &raw));

        let toks = vec!["gmail".to_string(), "messages".to_string()];
        let mut i = empty_input();
        i.fuzzy = &toks;
        assert!(Filter::build(i).unwrap().matches(&rec, &raw));
        let toks = vec!["absent".to_string()];
        let mut i = empty_input();
        i.fuzzy = &toks;
        assert!(!Filter::build(i).unwrap().matches(&rec, &raw));

        for id in ["rec-1", "inv-9"] {
            let mut i = empty_input();
            i.id = Some(id);
            assert!(Filter::build(i).unwrap().matches(&rec, &raw), "id {id}");
        }
        let mut i = empty_input();
        i.id = Some("other");
        assert!(!Filter::build(i).unwrap().matches(&rec, &raw));
    }

    #[test]
    fn since_filters_by_recency() {
        let raw = "{}";
        let mut past = rec_http();
        past.timestamp = "2000-01-01T00:00:00.000Z".to_string();
        let mut future = rec_http();
        future.timestamp = "2999-01-01T00:00:00.000Z".to_string();
        let mut undated = rec_http();
        undated.timestamp = String::new();

        let mut i = empty_input();
        i.since = Some("1d");
        let f = Filter::build(i).unwrap();
        assert!(!f.matches(&past, raw));
        assert!(f.matches(&future, raw));
        assert!(
            !f.matches(&undated, raw),
            "unparseable timestamp is excluded"
        );
    }

    #[test]
    fn query_covers_every_field_arm() {
        let mut rec = rec_http();
        rec.source = Some(Source::Mcp);
        rec.mcp_tool = Some("gmail_search".to_string());
        rec.error = Some("boom timeout".to_string());
        rec.command = vec!["gmail".to_string(), "read".to_string()];
        let raw = serde_json::to_string(&rec).unwrap().to_ascii_lowercase();

        let cases = [
            ("kind:http", true),
            ("kind:invocation", false),
            ("source:mcp", true),
            ("source:cli", false),
            ("service:gmail", true),
            ("method:GET", true),
            ("status:2xx", true),
            ("status:5xx", false),
            ("command:gmail", true),
            ("cmd:\"gmail read\"", true),
            ("url:messages", true),
            ("id:rec-1", true),
            ("id:inv-9", true),
            ("id:nope", false),
            ("inv:inv-9", true),
            ("invocation_id:inv-9", true),
            ("tool:gmail_search", true),
            ("mcp_tool:other", false),
            ("error:timeout", true),
            ("err:absent", false),
            ("error:", true),
            ("unknownfield:x", false),
        ];
        for (q, expected) in cases {
            let parsed = parse_query(q).unwrap();
            assert_eq!(parsed.eval(&rec, &raw), expected, "query: {q}");
        }
    }

    #[test]
    fn query_unknown_field_falls_back_to_context_map() {
        // Drive-mutation fields live in the free-form `context` map; an
        // unknown field name looks them up there (case-insensitive substring).
        let mut rec = LogRecord {
            kind: RecordKind::DriveMutation,
            ..LogRecord::default()
        };
        rec.context
            .insert("file_id".to_string(), "1AbC-file".to_string());
        rec.context
            .insert("file_name".to_string(), "Quarterly report.pdf".to_string());
        let raw = serde_json::to_string(&rec).unwrap().to_ascii_lowercase();

        let cases = [
            ("file_id:1abc", true),
            ("file_id:1ABC-FILE", true),
            ("file_id:other-file", false),
            ("file_name:report", true),
            ("lease_id:abc", false), // key absent from context
        ];
        for (q, expected) in cases {
            let parsed = parse_query(q).unwrap();
            assert_eq!(parsed.eval(&rec, &raw), expected, "query: {q}");
        }
    }

    #[test]
    fn query_status_is_kind_aware_for_drivemutation() {
        // A `drivemutation` record has no `status_code`; its domain status
        // lives in `context["status"]` and matches by exact case-insensitive
        // equality, not substring (see #1624).
        let mut rec = LogRecord {
            kind: RecordKind::DriveMutation,
            ..LogRecord::default()
        };
        rec.context
            .insert("status".to_string(), "blocked".to_string());
        let raw = serde_json::to_string(&rec).unwrap().to_ascii_lowercase();

        let cases = [
            ("status:blocked", true),
            ("status:BLOCKED", true),
            ("status:written", false),
            ("status:block", false), // exact match, not substring
        ];
        for (q, expected) in cases {
            let parsed = parse_query(q).unwrap();
            assert_eq!(parsed.eval(&rec, &raw), expected, "query: {q}");
        }
    }

    #[test]
    fn query_parser_edge_cases() {
        let rec = LogRecord::default();
        let raw = r#"{"x":"hello world"}"#.to_ascii_lowercase();

        assert!(parse_query("\"hello world\"").unwrap().eval(&rec, &raw));
        assert!(parse_query("hello AND world").unwrap().eval(&rec, &raw));
        assert!(!parse_query("NOT hello").unwrap().eval(&rec, &raw));
        assert!(parse_query("(hello OR nope) AND world")
            .unwrap()
            .eval(&rec, &raw));

        assert!(parse_query("(hello").is_err());
        assert!(parse_query("hello )").is_err());
        assert!(parse_query("").is_err());
    }

    #[test]
    fn matches_rejects_on_each_clause() {
        let mut rec = rec_http();
        rec.command = vec!["gmail".to_string(), "read".to_string()];
        let raw = serde_json::to_string(&rec).unwrap();

        // Each clause, set to a value the record does NOT satisfy, fails the match.
        let mut status = empty_input();
        status.status = Some("5xx");
        assert!(!Filter::build(status).unwrap().matches(&rec, &raw));

        let mut service = empty_input();
        service.service = Some("drive");
        assert!(!Filter::build(service).unwrap().matches(&rec, &raw));

        let mut command = empty_input();
        command.command = Some("git");
        assert!(!Filter::build(command).unwrap().matches(&rec, &raw));
        // …and the matching command passes.
        let mut command = empty_input();
        command.command = Some("gmail");
        assert!(Filter::build(command).unwrap().matches(&rec, &raw));

        let mut url = empty_input();
        url.url = Some("absent-path");
        assert!(!Filter::build(url).unwrap().matches(&rec, &raw));

        let mut grep = empty_input();
        grep.grep = Some("ZZZ-not-present");
        assert!(!Filter::build(grep).unwrap().matches(&rec, &raw));

        // A --query clause that fails also rejects the record.
        let q = vec!["service:drive".to_string()];
        let mut query = empty_input();
        query.query = &q;
        assert!(!Filter::build(query).unwrap().matches(&rec, &raw));
    }

    #[test]
    fn since_rejects_all_digit_and_empty_number() {
        // No unit (all digits) hits the "no non-digit found" error path.
        let mut all_digits = empty_input();
        all_digits.since = Some("30");
        assert!(Filter::build(all_digits).is_err());

        // A leading non-digit yields an empty number, hitting the parse error.
        let mut empty_num = empty_input();
        empty_num.since = Some("xh");
        assert!(Filter::build(empty_num).is_err());
    }

    #[test]
    fn query_covers_kind_source_and_error_variants() {
        let raw = "{}".to_ascii_lowercase();

        for (kind, q, want) in [
            (RecordKind::Invocation, "kind:invocation", true),
            (RecordKind::Http, "kind:invocation", false),
            (RecordKind::Unknown, "kind:unknown", true),
        ] {
            let rec = LogRecord {
                kind,
                ..LogRecord::default()
            };
            assert_eq!(parse_query(q).unwrap().eval(&rec, &raw), want, "{q}");
        }

        for (source, q, want) in [
            (Source::Cli, "source:cli", true),
            (Source::Unknown, "source:daemon", false),
            (Source::Unknown, "source:unknown", true),
            (Source::Mcp, "source:cli", false),
        ] {
            let rec = LogRecord {
                source: Some(source),
                ..LogRecord::default()
            };
            assert_eq!(parse_query(q).unwrap().eval(&rec, &raw), want, "{q}");
        }

        // The error arm with no error present returns false.
        let rec = LogRecord::default();
        assert!(!parse_query("error:boom").unwrap().eval(&rec, &raw));
    }

    #[test]
    fn numeric_match_operators() {
        assert!(numeric_match(Some(1500), ">1000"));
        assert!(!numeric_match(Some(500), ">1000"));
        assert!(numeric_match(Some(1000), ">=1000"));
        assert!(numeric_match(Some(200), "<500"));
        assert!(numeric_match(Some(500), "<=500"));
        assert!(numeric_match(Some(42), "42")); // bare == equality
        assert!(numeric_match(Some(42), "=42"));
        assert!(!numeric_match(Some(43), "42"));
        // Absent field and unparseable spec never match.
        assert!(!numeric_match(None, ">0"));
        assert!(!numeric_match(Some(1), ">abc"));
    }

    #[test]
    fn query_covers_numeric_fields_and_comparators() {
        let rec = LogRecord {
            kind: RecordKind::Invocation,
            exit_code: Some(1),
            duration_ms: Some(1500),
            status_code: Some(503),
            elapsed_ms: Some(2500),
            ..LogRecord::default()
        };
        let raw = serde_json::to_string(&rec).unwrap().to_ascii_lowercase();
        let cases = [
            ("exit_code:>0", true),   // failed runs
            ("exit:1", true),         // alias + equality
            ("exit_code:0", false),   //
            ("duration:>1000", true), // slow invocations
            ("dur:<1000", false),
            ("duration_ms:1500", true),
            ("elapsed:>1000", true), // slow requests
            ("elapsed_ms:<=2500", true),
            ("status:>=500", true), // comparator on status
            ("status:<500", false),
            ("status:5xx", true), // class syntax still works
        ];
        for (q, want) in cases {
            assert_eq!(parse_query(q).unwrap().eval(&rec, &raw), want, "{q}");
        }
    }

    #[test]
    fn query_covers_text_fields() {
        let rec = LogRecord {
            hostname: "build-box-01".to_string(),
            system_user: "ci-runner".to_string(),
            cwd: "/home/ci/project".to_string(),
            auth_principal: Some("token-abc".to_string()),
            ..LogRecord::default()
        };
        let raw = serde_json::to_string(&rec).unwrap().to_ascii_lowercase();
        let cases = [
            ("hostname:build-box", true),
            ("host:BUILD-BOX-01", true), // alias + case-insensitive
            ("host:other", false),
            ("system_user:ci-runner", true),
            ("user:ci", true),
            ("cwd:/home/ci", true),
            ("auth_principal:token-abc", true),
            ("principal:abc", true),
            ("principal:nope", false),
        ];
        for (q, want) in cases {
            assert_eq!(parse_query(q).unwrap().eval(&rec, &raw), want, "{q}");
        }
        // An absent auth_principal never matches.
        let bare = LogRecord::default();
        assert!(!parse_query("principal:x").unwrap().eval(&bare, "{}"));
    }

    #[test]
    fn parse_time_bound_accepts_absolute_and_relative() {
        // RFC3339 timestamp.
        let ts = parse_time_bound("2026-07-01T12:00:00Z").unwrap();
        assert_eq!(ts.to_rfc3339(), "2026-07-01T12:00:00+00:00");
        // Date-only → midnight UTC.
        let date = parse_time_bound("2026-07-01").unwrap();
        assert_eq!(date.to_rfc3339(), "2026-07-01T00:00:00+00:00");
        // Relative still works and is in the past.
        assert!(parse_time_bound("2h").unwrap() < Utc::now());
        // Garbage is rejected.
        assert!(parse_time_bound("not-a-time").is_err());
    }

    #[test]
    fn until_bounds_the_upper_window() {
        let raw = "{}";
        let mut old = rec_http();
        old.timestamp = "2026-06-01T00:00:00.000Z".to_string();
        let mut recent = rec_http();
        recent.timestamp = "2026-07-10T00:00:00.000Z".to_string();

        // --until as an absolute upper bound: keep old, drop recent.
        let mut i = empty_input();
        i.until = Some("2026-07-01T00:00:00Z");
        let f = Filter::build(i).unwrap();
        assert!(f.matches(&old, raw));
        assert!(!f.matches(&recent, raw));

        // Bounded window with both ends.
        let mut i = empty_input();
        i.since = Some("2026-06-15");
        i.until = Some("2026-07-05");
        let f = Filter::build(i).unwrap();
        assert!(!f.matches(&old, raw), "before --since");
        assert!(!f.matches(&recent, raw), "after --until");
    }

    fn drive_rec(status: &str) -> LogRecord {
        let mut rec = LogRecord {
            kind: RecordKind::DriveMutation,
            ..LogRecord::default()
        };
        rec.context.insert("status".to_string(), status.to_string());
        rec.context
            .insert("file_id".to_string(), "1AbC".to_string());
        rec
    }

    fn filter_for(status: Option<&str>, query: &[&str]) -> Result<Filter> {
        let query: Vec<String> = query.iter().map(|q| (*q).to_string()).collect();
        let mut input = empty_input();
        input.status = status;
        input.query = &query;
        Filter::build(input)
    }

    #[test]
    fn builtin_fields_list_matches_the_match_arms() {
        for name in BUILTIN_FIELDS {
            assert!(is_builtin_field(name), "{name} is listed but not handled");
            assert!(is_builtin_field(&name.to_ascii_uppercase()), "{name}");
        }
        assert!(!is_builtin_field("via_daemon"));
        assert!(!is_builtin_field("file_id"));
        assert!(!is_builtin_field("servce"));
        assert!(!is_builtin_field("12"));
    }

    #[test]
    fn legacy_daemon_query_uses_context_fallback_and_warns() {
        let raw = r#"{"kind":"http","source":"daemon","via_daemon":true,
            "daemon_session_id":"legacy-session"}"#;
        let mut rec: LogRecord = serde_json::from_str(raw).unwrap();
        assert!(parse_query("source:unknown").unwrap().eval(&rec, raw));
        let filter = filter_for(None, &["via_daemon:true"]).unwrap();
        assert!(!filter.matches(&rec, raw));
        let warnings = filter.unknown_field_warnings(false);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("`via_daemon`"));
        rec.context
            .insert("via_daemon".to_string(), "true".to_string());
        assert!(filter.matches(&rec, raw));
        assert!(filter.unknown_field_warnings(false).is_empty());
    }

    #[test]
    fn edit_distance_counts_single_edits() {
        assert_eq!(edit_distance("service", "service"), 0);
        assert_eq!(edit_distance("servce", "service"), 1);
        assert_eq!(edit_distance("sevrice", "service"), 2);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("abc", ""), 3);
    }

    #[test]
    fn typo_field_warns_with_a_suggestion() {
        let f = filter_for(None, &["servce:drive"]).unwrap();
        let rec = drive_rec("blocked");
        assert!(!f.matches(&rec, "{}"));
        let warnings = f.unknown_field_warnings(false);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let w = &warnings[0];
        assert!(w.contains("`servce`"), "{w}");
        assert!(w.contains("Did you mean `service`?"), "{w}");
        assert!(w.contains("the 1 record scanned"), "{w}");
        assert!(w.contains("\"servce:drive\""), "{w}");
    }

    #[test]
    fn time_like_token_warns_without_a_suggestion() {
        let f = filter_for(None, &["12:34:56"]).unwrap();
        f.matches(&drive_rec("blocked"), "{}");
        f.matches(&rec_http(), "{}");
        let warnings = f.unknown_field_warnings(false);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("any of the 2 records scanned"));
        assert!(!warnings[0].contains("Did you mean"), "{}", warnings[0]);
    }

    #[test]
    fn typo_of_a_seen_context_key_suggests_that_key() {
        let f = filter_for(None, &["file_idd:1abc"]).unwrap();
        f.matches(&drive_rec("blocked"), "{}");
        let warnings = f.unknown_field_warnings(false);
        assert!(
            warnings[0].contains("Did you mean `file_id`?"),
            "{warnings:?}"
        );
    }

    #[test]
    fn real_fields_and_seen_context_keys_do_not_warn() {
        let f = filter_for(
            None,
            &["file_id:1abc service:drive OR NOT (status:blocked AND File_ID:x)"],
        )
        .unwrap();
        f.matches(&drive_rec("blocked"), "{}");
        assert!(f.unknown_field_warnings(false).is_empty());
    }

    #[test]
    fn a_field_seen_in_a_later_record_stops_warning() {
        let f = filter_for(None, &["file_id:1abc"]).unwrap();
        f.matches(&rec_http(), "{}");
        f.matches(&drive_rec("blocked"), "{}");
        assert!(f.unknown_field_warnings(false).is_empty());
    }

    #[test]
    fn nothing_scanned_means_nothing_to_warn_about() {
        let f = filter_for(None, &["servce:drive"]).unwrap();
        assert!(f.unknown_field_warnings(false).is_empty());
    }

    #[test]
    fn no_context_terms_means_no_warnings() {
        let f = filter_for(Some("5xx"), &["status:5xx deploy"]).unwrap();
        f.matches(&rec_http(), "{}");
        assert!(f.unknown_field_warnings(false).is_empty());
    }

    #[test]
    fn a_quoted_word_is_a_literal_not_an_operator() {
        let rec = LogRecord::default();
        let raw = "an error: not found, or maybe and";
        for word in ["not", "or", "and", "NOT"] {
            let q = format!("\"{word}\"");
            assert!(parse_query(&q).unwrap().eval(&rec, raw), "{q}");
            assert!(!parse_query(&q).unwrap().eval(&rec, "all fine here"), "{q}");
        }
        // Unquoted, `not` is still the operator.
        assert!(parse_query("error not found").unwrap().eval(&rec, "error"));
        assert!(!parse_query("error not found")
            .unwrap()
            .eval(&rec, "error found"));
        // Literals compose with operators and negation.
        assert!(parse_query("error \"not\" found")
            .unwrap()
            .eval(&rec, "error not found"));
        assert!(parse_query("-\"not\"").unwrap().eval(&rec, "all fine"));
    }

    #[test]
    fn an_empty_quoted_term_is_rejected_not_match_all() {
        assert!(parse_query("\"\"").is_err());
        assert!(parse_query("deploy \"\"").is_err());
        // An empty quoted *value* in a field term is still allowed.
        assert!(parse_query("error:\"\"").is_ok());
    }

    #[test]
    fn following_warnings_say_so_far() {
        let f = filter_for(None, &["servce:drive"]).unwrap();
        f.matches(&drive_rec("blocked"), "{}");
        assert!(f.unknown_field_warnings(true)[0].contains("matches nothing so far."));
        assert!(f.unknown_field_warnings(false)[0].contains("matches nothing."));
    }

    fn status_warnings(status: Option<&str>, query: &[&str], recs: &[LogRecord]) -> Vec<String> {
        let f = filter_for(status, query).unwrap();
        for rec in recs {
            f.matches(rec, "{}");
        }
        f.unseen_status_warnings(false)
    }

    #[test]
    fn typo_status_warns_with_a_suggestion_for_flag_and_query() {
        let recs = [drive_rec("blocked"), drive_rec("written")];
        let flag = status_warnings(Some("blokced"), &[], &recs);
        assert_eq!(flag.len(), 1, "{flag:?}");
        assert!(flag[0].contains("`blokced`"), "{flag:?}");
        assert!(flag[0].contains("--status blokced"), "{flag:?}");
        assert!(
            flag[0].contains("any of the 2 drivemutation records"),
            "{flag:?}"
        );
        assert!(flag[0].contains("Did you mean `blocked`?"), "{flag:?}");

        let query = status_warnings(None, &["status:Blokced"], &recs);
        assert_eq!(query.len(), 1, "{query:?}");
        assert!(query[0].contains("`status:Blokced`"), "{query:?}");
        assert!(query[0].contains("Did you mean `blocked`?"), "{query:?}");
    }

    #[test]
    fn the_same_unseen_status_word_warns_once() {
        let recs = [drive_rec("blocked")];
        let w = status_warnings(Some("blokced"), &["status:BLOKCED"], &recs);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("--status blokced"), "{w:?}");
    }

    #[test]
    fn an_absent_but_valid_status_warns_without_a_suggestion() {
        let w = status_warnings(Some("blocked"), &[], &[drive_rec("written")]);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("the 1 drivemutation record scanned"), "{w:?}");
        assert!(!w[0].contains("Did you mean"), "{w:?}");
    }

    #[test]
    fn no_drivemutation_record_scanned_stays_quiet() {
        assert!(status_warnings(Some("blokced"), &[], &[]).is_empty());
        assert!(status_warnings(Some("blokced"), &[], &[rec_http()]).is_empty());
        assert!(status_warnings(None, &["status:blokced"], &[rec_http()]).is_empty());
    }

    #[test]
    fn a_drivemutation_record_without_a_status_counts_but_offers_no_hint() {
        let bare = LogRecord {
            kind: RecordKind::DriveMutation,
            ..LogRecord::default()
        };
        let w = status_warnings(Some("blocked"), &[], &[bare]);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("the 1 drivemutation record scanned"), "{w:?}");
        assert!(!w[0].contains("Did you mean"), "{w:?}");
    }

    #[test]
    fn a_status_seen_in_any_record_stops_the_warning() {
        let recs = [rec_http(), drive_rec("written"), drive_rec("BLOCKED")];
        assert!(status_warnings(Some("blocked"), &[], &recs).is_empty());
        assert!(status_warnings(None, &["status:Blocked"], &recs).is_empty());
    }

    #[test]
    fn a_status_list_warns_only_for_the_missing_words() {
        let recs = [drive_rec("written")];
        let w = status_warnings(Some("written, blokced"), &[], &recs);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("`blokced`"), "{w:?}");
    }

    #[test]
    fn numeric_statuses_and_other_fields_are_not_watched() {
        let recs = [drive_rec("written")];
        for status in ["200", "5xx", "4xx,5xx", ">=400"] {
            assert!(
                status_warnings(Some(status), &[], &recs).is_empty(),
                "{status}"
            );
            let q = format!("status:{status}");
            assert!(status_warnings(None, &[&q], &recs).is_empty(), "{q}");
        }
        assert!(status_warnings(None, &["verdict:blokced \"status:blokced\""], &recs).is_empty());
    }

    #[test]
    fn only_status_words_in_a_mixed_list_are_watched() {
        let recs = [drive_rec("written")];
        let w = status_warnings(Some("written,5xx,500"), &["status:written,>=400"], &recs);
        assert!(w.is_empty(), "{w:?}");
        let w = status_warnings(Some("blokced,5xx"), &[], &recs);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("`blokced`"), "{w:?}");
    }

    #[test]
    fn a_negated_or_nested_status_term_is_watched() {
        let recs = [drive_rec("written")];
        let w = status_warnings(None, &["service:drive OR NOT (status:blokced)"], &recs);
        assert_eq!(w.len(), 1, "{w:?}");
    }

    #[test]
    fn following_status_warnings_say_so_far() {
        let f = filter_for(Some("blokced"), &[]).unwrap();
        f.matches(&drive_rec("blocked"), "{}");
        assert!(f.unseen_status_warnings(true)[0].contains("matches no record so far."));
        assert!(f.unseen_status_warnings(false)[0].contains("matches no record."));
    }

    #[test]
    fn a_quoted_word_is_never_a_field_term() {
        let rec = http(Some(500), "gmail", "GET");
        // As a field term `status:5xx` would match; as text it does not.
        assert!(!parse_query("\"status:5xx\"").unwrap().eval(&rec, "{}"));
        assert!(parse_query("\"12:34:56\"")
            .unwrap()
            .eval(&rec, "at 12:34:56 utc"));
        // A quote inside a `field:value` word keeps its field meaning.
        let rec = LogRecord {
            cwd: "/tmp/a b".to_string(),
            ..LogRecord::default()
        };
        assert!(parse_query("cwd:\"a b\"").unwrap().eval(&rec, "{}"));
        // A quoted literal raises no unknown-field warning.
        let f = filter_for(None, &["\"12:34:56\""]).unwrap();
        f.matches(&drive_rec("blocked"), "{}");
        assert!(f.unknown_field_warnings(false).is_empty());
    }

    #[test]
    fn status_flag_accepts_drive_mutation_statuses_like_the_query_field() {
        let blocked = drive_rec("blocked");
        let written = drive_rec("written");
        let would = drive_rec("would-write");
        let flag = |s: &str, rec: &LogRecord| {
            let f = filter_for(Some(s), &[]).unwrap();
            f.matches(rec, "{}")
        };
        assert!(flag("blocked", &blocked));
        assert!(flag("BLOCKED", &blocked));
        assert!(!flag("blocked", &written));
        assert!(flag("blocked,written", &written));
        assert!(flag("blocked, written", &written));
        // Exact, not substring.
        assert!(!flag("written", &would));
        // Not a Drive-mutation record: no domain status to match.
        assert!(!flag("blocked", &rec_http()));
        assert!(!flag("5xx", &blocked));
    }

    #[test]
    fn status_flag_and_status_query_agree() {
        let mut records = vec![drive_rec("blocked"), drive_rec("written"), rec_http()];
        for code in [200, 404, 503] {
            records.push(http(Some(code), "gmail", "GET"));
        }
        for spec in [
            "blocked",
            "blocked,written",
            "200",
            "5xx",
            "4xx,5xx",
            ">=400",
            "<300",
            "=404",
        ] {
            let flag = filter_for(Some(spec), &[]).unwrap();
            let query = filter_for(None, &[&format!("status:{spec}")]).unwrap();
            for rec in &records {
                assert_eq!(
                    flag.matches(rec, "{}"),
                    query.matches(rec, "{}"),
                    "--status {spec} vs status:{spec} on {rec:?}"
                );
            }
        }
    }

    #[test]
    fn status_flag_still_rejects_malformed_numeric_forms() {
        for bad in ["9xx", "", "4xx,blocked", ">=abc", ">", "!!", "20x"] {
            assert!(filter_for(Some(bad), &[]).is_err(), "{bad:?}");
        }
        for good in [
            "200",
            "5xx",
            "4xx,5xx",
            ">=400",
            "blocked",
            "stale-revision",
        ] {
            assert!(filter_for(Some(good), &[]).is_ok(), "{good:?}");
        }
    }
}

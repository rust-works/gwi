# Style Guide

Conventions for code, documentation, and other project artifacts in the gwi project.

> **Provenance:** seeded from omni-dev's `docs/STYLE_GUIDE.md` (rust-works/omni-dev#2203, Phase 0)
> and pruned in #47 to the subsystems gwi has (Gmail, Drive, MCP, shared utilities). Rule IDs are
> kept from omni-dev so existing references resolve; a rule ID is never reused or renumbered (the numbered
> items inside a rule may be, as in STYLE-0026). Where a rule cites an omni-dev issue, that is omni-dev's
> issue number.

Each item has a unique ID for easy reference.

## Tag-based lookup

Before writing or reviewing code, documentation, or other project artifacts, identify
which tags apply to the changes and search this file for those tags. Each rule has a
**Tags** line immediately after its heading.

**Search command:** `grep "Tags:.*<tag>" docs/STYLE_GUIDE.md` returns matching rule headings.

| When you are…                                   | Search for tags                                                 |
|-------------------------------------------------|-----------------------------------------------------------------|
| Adding or modifying a function                  | `code-style`, `naming`, `documentation`                         |
| Adding a type, enum, or trait                   | `api-design`, `naming`, `documentation`                         |
| Adding or changing error handling               | `error-handling`                                                |
| Creating or restructuring a module/file         | `module-organization`, `naming`                                 |
| Writing or updating tests                       | `testing`                                                       |
| Adding a new Gmail or Drive client method       | `testing`, `api-design`                                         |
| Adding a new CLI command                        | `testing`, `module-organization`                                |
| Changing visibility (`pub`, `pub(crate)`)       | `api-design`, `module-organization`                             |
| Adding constants or replacing magic values      | `code-style`, `naming`                                          |
| Writing commit messages                         | `commits`                                                       |
| After creating commits (before push / PR)       | `commits`                                                       |
| Suppressing a lint or considering `unsafe`      | `code-style`, `unsafe`                                          |
| Writing or updating an ADR                      | `adrs`                                                          |
| Adding an MCP tool or param struct              | `api-design`, `module-organization`, `testing`, `documentation` |
| Adding or modifying a docs/plan/ file           | `documentation`, `adrs`                                         |
| Reading env vars, or testing env-dependent code | `testing`, `module-organization`                                |
| Resolving a default writable state-directory path | `testing`, `module-organization`                              |
| Reading a credential or other secret            | `module-organization`, `api-design`, `testing`                  |
| Reviewing code for style compliance             | All tags relevant to the changed code                           |

---

## STYLE-0000: Style guide structure

**Tags:** `meta`

### Situation

A new convention needs to be added to this style guide.

### Guidance

Assign the next sequential ID (currently next is `STYLE-0032`; IDs of removed rules are
retired, not reused) and include:

1. A **Tags** line immediately after the heading — a comma-separated list of category labels
   from the tag vocabulary below.
2. Three subheadings:
   - **Situation** — when this rule applies
   - **Guidance** — what to do (with examples where helpful)
   - **Motivation** — why this rule exists

**Tag vocabulary** (extend as needed):

| Tag                  | Covers                                             |
|----------------------|----------------------------------------------------|
| `meta`               | Style guide structure and process                  |
| `error-handling`     | Error types, context messages, panics, suppression |
| `module-organization`| File layout, visibility, cohesion                  |
| `naming`             | Naming conventions for types, functions, files     |
| `commits`            | Commit message format, scope rules, discipline     |
| `documentation`      | Doc comments, examples                             |
| `testing`            | Test structure, fixtures, snapshots                |
| `code-style`         | Imports, clippy, constants, function length        |
| `api-design`         | Ownership, must_use, type safety, string params    |
| `unsafe`             | Unsafe code policy                                 |
| `adrs`               | Architecture Decision Record format and process    |

A rule may have **multiple tags** — e.g., a rule about error messages in tests could be
tagged `error-handling, testing`.

Items are ordered by ID. **Do not** group items under section headings; use tags for
categorisation instead.

### Motivation

Consistent structure makes the guide scannable, and stable IDs allow code review comments
and ADRs to reference specific rules unambiguously. Tags replace section headings so that
items can remain in strict ID order without needing to be shuffled between sections when
categories overlap or new categories are introduced.

---

## STYLE-0001: Default error type

**Tags:** `error-handling`

### Situation

A function can fail and needs to return an error.

### Guidance

Use `anyhow::Result<T>` as the return type. Import both `Context` and `Result`:

```rust
use anyhow::{Context, Result};

fn load_settings(path: &Path) -> Result<Settings> {
    let content = fs::read_to_string(path).context("Failed to read settings file")?;
    // ...
}
```

Reserve `thiserror` enums for domain boundaries where callers need to match on specific
error variants. The custom error types are `DriveError` in
[`src/drive/error.rs`](../src/drive/error.rs), `GmailError` in
[`src/gmail/error.rs`](../src/gmail/error.rs) and `SecretEnvError` in
[`src/utils/secret_env.rs`](../src/utils/secret_env.rs), which cover API-specific failure
modes (credentials not configured, a failed API request, an unreadable secret file). These
convert to `anyhow::Error` automatically via the blanket impl.

Use `anyhow::bail!()` for early returns with an error message:

```rust
anyhow::bail!("Sheet range must not be empty");
```

### Motivation

`anyhow` provides lightweight error chaining without defining boilerplate error types.
Reserving `thiserror` for domain boundaries keeps the type surface small while still
allowing pattern matching where it matters.

---

## STYLE-0002: Context message style

**Tags:** `error-handling`

### Situation

Adding `.context()` or `.with_context()` to a fallible operation.

### Guidance

Write context messages in **sentence case** describing the **failed operation**:

```rust
// Good — describes the operation that failed
.context("Failed to read settings file")?;
.context("Cannot write to a read-only Drive account")?;
.context("Not signed in to Gmail")?;

// Bad — includes function name
.context("load_settings: could not open")?;

// Bad — too generic
.context("error")?;
```

Use `.with_context()` when the message needs runtime values:

```rust
.with_context(|| format!("Failed to parse account name: {}", name))?;
```

Prefer `.context()` over `.with_context()` for static messages since it avoids the closure
allocation.

### Motivation

Sentence-case messages read naturally in error chains printed by `main.rs`. Describing the
operation (not the function) keeps messages useful regardless of refactoring. The
`with_context` pattern avoids allocating format strings on the success path.

---

## STYLE-0003: Panicking operations

**Tags:** `error-handling`

### Situation

Considering `unwrap()`, `expect()`, or other panicking calls.

### Guidance

**`unwrap()` is acceptable** in these cases only:

- **Static regex** — use `std::sync::LazyLock` so the pattern is compiled once and the
  `unwrap()` is confined to the initialiser. Clippy's `invalid_regex` lint (deny by default)
  validates the literal at compile time, so the `unwrap()` is provably safe.

  ```rust
  use std::sync::LazyLock;
  use regex::Regex;

  static SCOPE_RE: LazyLock<Regex> =
      LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9-]*$").unwrap());
  ```

- **Known-safe constructors** — `FixedOffset::east_opt(0).unwrap()` where the argument is
  a constant that cannot fail.
- **Test code** — tests may use `unwrap()` freely.

**`expect()` is acceptable** for truly catastrophic I/O that should terminate the process:

```rust
io::stdout().flush().expect("Failed to flush stdout");
```

**Never** use `unwrap()` or `expect()` on user-supplied or runtime data in library code.
Use `?` with `.context()` instead.

### Motivation

Panics in library code produce poor diagnostics and cannot be handled by callers. Limiting
panics to provably-safe or catastrophic cases keeps the error surface predictable.
A lazy static avoids recompiling the regex on every call and makes the safety argument
obvious at the declaration site.

---

## STYLE-0004: Module file layout

**Tags:** `module-organization`

### Situation

Adding a new module or reorganizing an existing one.

### Guidance

Use the **named-file layout** (Rust 2018+) for modules with submodules. Place the parent
module in a file named after the module alongside a directory of the same name:

```
src/
├── drive.rs            # declares submodules
├── drive/
│   ├── client.rs
│   ├── error.rs
│   ├── docs.rs         # declares docs submodules
│   ├── docs/
│   │   ├── anchor.rs
│   │   ├── api.rs
│   │   └── client.rs
│   ├── lease.rs        # declares lease submodules
│   └── lease/
│       ├── acquire.rs
│       ├── check.rs
│       └── ledger.rs
├── request_log.rs      # no submodules, so just a single file
├── lib.rs
└── main.rs
```

Do **not** use `mod.rs` for new modules. The named-file layout gives every module root a
unique filename, which avoids ambiguous editor tabs and search results when multiple
`mod.rs` files exist.

Re-export key public types from each module root so consumers can import from the parent
module:

```rust
// src/mcp.rs
pub use error::tool_error;
pub use server::GwiServer;
pub use truncate::{truncate_response, DEFAULT_MAX_RESPONSE_BYTES};
```

Only re-export types that appear in the module's public API signatures. Internal helpers,
intermediate types, and implementation details should stay private to their submodule even
if they are `pub` there. A re-export is a promise that the type is part of the module's
contract.

### Motivation

The named-file layout is recommended by the Rust Book and is the default assumed by
`rust-analyzer`. Each module root has a distinct filename (e.g., `drive.rs` vs `lease.rs`)
instead of multiple `mod.rs` files, making editor tabs, file search, and `git log` output
unambiguous. Re-exports in the module root present a clean public interface per module.
Limiting re-exports to API-surface types prevents leaking implementation details that would
be hard to remove later.

---

## STYLE-0005: Visibility

**Tags:** `module-organization`, `api-design`

### Situation

Deciding whether to make an item `pub`, `pub(crate)`, or private.

### Guidance

Default to **private** (no visibility modifier). Use three visibility levels:

| Visibility   | Meaning                  | Use when                                               |
|--------------|--------------------------|--------------------------------------------------------|
| *(none)*     | Private to the module    | Internal helpers                                       |
| `pub(crate)` | Visible within the crate | Shared across modules but not part of the external API |
| `pub`        | Fully public             | Part of the crate's published API surface              |

```rust
impl DriveClient {
    pub fn from_credentials(credentials: &DriveCredentials) -> Result<Self> { ... }  // public API
    pub(crate) fn from_credentials_with(/* ... */) -> Result<Self> { ... }         // crate-internal
    pub(in crate::drive) fn transport(&self) -> &GoogleApiClient { ... }            // one subsystem
}
```

When in doubt, start private and widen visibility only when needed. Prefer `pub(crate)`
over `pub` for items that other modules need but external consumers should not rely on.

The rustc lint `unreachable_pub` (allowed by default) can be enabled to detect `pub` items
that are not actually reachable from outside the crate.

### Motivation

Minimal visibility reduces the API surface that must be maintained (Effective Rust, Item
22). Using `pub(crate)` for internal cross-module items prevents accidentally promising API
stability to external consumers. Making a public item private is a breaking change; making
a private item public is not.

---

## STYLE-0006: Naming patterns

**Tags:** `naming`

### Situation

Naming a new type, function, CLI command, environment variable, or YAML field.

### Guidance

| Element           | Convention            | Examples                                      |
|-------------------|-----------------------|-----------------------------------------------|
| Structs / Enums   | PascalCase            | `DriveClient`, `GmailError`, `OutputFormat`   |
| Traits            | PascalCase (adj/verb) | `EnvSource`, `Serialize`, `Display`           |
| Functions/Methods | snake_case            | `create_client_from()`, `tool_error()`        |
| Type aliases      | PascalCase            | `Result<T>` (for crate-local aliases)         |
| Constants         | UPPER_SNAKE_CASE      | `VERSION`, `DEFAULT_MAX_RESPONSE_BYTES`       |
| Environment vars  | UPPER_SNAKE_CASE      | `GWI_CONFIG_DIR`, `DRIVE_REFRESH_TOKEN`       |
| CLI commands      | kebab-case            | `help-all`, `drive sheets append`             |
| MCP tools         | snake_case            | `gmail_search`, `drive_docs_replace`          |
| YAML fields       | snake_case            | `require_lease`, `refresh_token`              |
| Modules / files   | snake_case            | `chrome_profile.rs`, `rate_limit.rs`          |

### Motivation

Standard Rust naming (`PascalCase` types, `snake_case` functions) is enforced by compiler
warnings and `clippy`. Kebab-case CLI commands follow `clap` conventions and are standard
across Unix tools.

---

## STYLE-0007: Commit message format

**Tags:** `commits`

### Situation

Writing a commit message.

### Guidance

Follow [`.omni-dev/commit-guidelines.md`](../.omni-dev/commit-guidelines.md) for the full
specification including types, scopes, subject line rules, body guidelines, and breaking
change conventions. omni-dev's
[`omni-dev-directory.md`](https://github.com/rust-works/omni-dev/blob/main/docs/omni-dev-directory.md#commit-guidelinesmd)
documents the file's format contract, validation behaviour, and how it is resolved relative
to local overrides and the global fallback.

The commit guidelines must themselves follow **Conventional Commits** and remain consistent
with the scope definitions in `.omni-dev/scopes.yaml`:

1. **Single source of truth** — `scopes.yaml` is the sole maintained inventory of this
   repository's project scope names and descriptions. Do not duplicate that inventory in
   `commit-guidelines.md`: the checker injects the resolved valid scope set automatically.
   Keep scope policy in the guidelines, including the ecosystem-default note
   (`cargo`, `core`, `lib`, `test` for a Rust project), multi-scope rules, and file-ownership
   exceptions.
2. **Examples** — every `<scope>` used in the `## Examples` section must be a scope that
   exists in `scopes.yaml`. Do not use scopes from other projects or hypothetical scopes.
3. **Tree coverage** — every tracked file under `src/` and `.github/` must be
   matched by some scope's `file_patterns` (or listed in the `allow:` list for files that
   legitimately belong to no subsystem). When a new subsystem or module facade lands,
   `scopes.yaml` must gain a pattern for it in the same change. Coverage is checked against
   the project scopes only, never the ecosystem `lib` scope's `src/**` catch-all, which would
   make the check vacuously true. Each `allow:` entry carries a one-line justification
   comment. The list can rot — an entry added to silence a failure looks just like a correct
   one — so keep it small and prefer a scope or a `file_patterns` entry; growing it is at
   least a visible diff in review, unlike a catch-all that absorbs new subsystems silently.

Commit subjects are checked on every pull request by
[`commit-lint.yml`](../.github/workflows/commit-lint.yml), which runs
`omni-dev git commit message lint` against these guidelines and `scopes.yaml`. Clause 3 is
checked with `omni-dev config scopes lint --root src --root .github`; that command is not yet
a CI step (#50), so run it by hand when a change adds or moves files under `src/` or
`.github/`.

### Motivation

Keeping the detailed commit specification in `.omni-dev/commit-guidelines.md` allows the AI
context system to consume it directly, avoiding duplication between this style guide and the
machine-readable guidelines.

Both `commit-guidelines.md` and `scopes.yaml` are injected into the AI prompt for commit
checking. If the two files list different scopes the AI receives contradictory instructions
and may incorrectly flag valid scopes as invalid — or accept scopes that no longer exist.

The list is hand-maintained, and in omni-dev it drifted by nine entries
([#1421](https://github.com/rust-works/omni-dev/issues/1421)) without producing a visible
failure: the judge happened to resolve the contradiction in favour of `scopes.yaml`. Relying
on that is a coin flip, which is why omni-dev checks the rule with a test. gwi has no such test
yet: clauses 1 and 2 are kept by review, so check them by hand when either file changes.

---

## STYLE-0008: Doc comments

**Tags:** `documentation`

### Situation

Adding or updating documentation on a module, type, or function.

### Guidance

**Module-level docs** — every module file starts with a `//!` comment:

```rust
//! Large-output handling for MCP tool responses.
```

**Item-level docs** — every public struct, enum, field, variant, and method gets `///`:

```rust
/// Represents a Drive file with its metadata.
pub struct DriveFile {
    /// Drive's opaque file id.
    pub id: String,
    /// The file's display name.
    pub name: String,
}
```

**Summary line style** — write in **third-person singular present indicative** per
[RFC 505](https://rust-lang.github.io/rfcs/0505-api-comment-conventions.html). Use full
sentences ending with a period:

```rust
/// Builds a client from already-resolved credentials.
pub fn create_client_from(credentials: DriveCredentials) -> Result<DriveClient> { ... }

/// Returns the API base URL (without trailing slash).
pub fn base_url(&self) -> &str { ... }
```

| Correct (third-person)         | Incorrect (imperative)        |
|--------------------------------|-------------------------------|
| `/// Returns the length.`      | `/// Return the length.`      |
| `/// Creates a new client.`    | `/// Create a new client.`    |
| `/// Parses the input string.` | `/// Parse the input string.` |

The crate-level lint `#![warn(missing_docs)]` in `src/lib.rs` will warn on any public item
missing a doc comment.

**`# Examples` sections** — public functions that are not self-explanatory should include a
doc example. These are compiled and run by `cargo test`, so they serve as both documentation
and regression tests:

```rust
/// Parses a relative duration such as `30m` into the cutoff `now - duration`.
///
/// # Examples
///
/// ```
/// let cutoff = gwi::utils::duration::parse_since("30m").unwrap();
/// assert!(cutoff < chrono::Utc::now());
/// ```
pub fn parse_since(s: &str) -> Result<DateTime<Utc>> { ... }
```

Doc examples are not required for trivial getters, builders, or `From`/`Into`
implementations where the behaviour is obvious from the type signature.

### Motivation

The third-person convention matches the Rust standard library and `rustdoc` output, where
doc summaries read as descriptions of what the item *does* (e.g., `Vec::push` — "Appends
an element to the back of a collection."). RFC 505 codifies this as the official Rust API
documentation style. `#![warn(missing_docs)]` turns documentation into a compile-time
obligation rather than an afterthought. Doc examples provide compile-tested usage patterns
and catch API regressions that unit tests might miss.

---

## STYLE-0009: Test structure

**Tags:** `testing`

### Situation

Writing a new test.

### Guidance

Place unit tests in a `#[cfg(test)] mod tests` block at the **end** of the source file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_error_flattens_to_message() {
        let mcp = tool_error(anyhow!("top-level failure"));
        assert!(mcp.message.contains("top-level failure"));
    }
}
```

**Naming pattern:** `<thing_being_tested>[_<condition>]` — omit the `test_` prefix since
the `#[test]` attribute and `tests` module already identify these as tests. Clippy's
`redundant_test_prefix` lint (restriction group) flags the prefix as redundant.

```rust
fn single_error_flattens_to_message() { ... }
fn list_propagates_api_errors() { ... }
fn create_client_from_uses_drive_api_host() { ... }
```

When a test uses `?` for error propagation, return `Result<()>`:

```rust
#[test]
fn load_settings_from_temp_dir() -> Result<()> {
    let dir = tempfile::TempDir::new()?;
    // ...
    Ok(())
}
```

Place integration tests in the `tests/` directory.

**Test attributes:**

- **`#[should_panic]`** — avoid in favour of `Result`-returning tests that assert on the
  error. `#[should_panic]` matches on panic message substrings which are brittle across
  refactors. Use it only when testing that a documented panic condition (e.g., an `expect()`
  from STYLE-0003) fires correctly.
- **`#[ignore]`** — acceptable for tests that require external resources (network, API keys)
  or are unusually slow. Always add a reason: `#[ignore = "requires a live Google account"]`. Run
  ignored tests explicitly with `cargo test -- --ignored`.

### Motivation

The `mod tests` convention is idiomatic Rust and gives tests access to private items via
`use super::*`. Dropping the `test_` prefix avoids the triple-redundancy of
`tests::test_foo` in `cargo test` output. Consistent naming makes
`cargo test list_propagates` filtering predictable.

---

## STYLE-0010: Test data and fixtures

**Tags:** `testing`

### Situation

A test needs temporary files, a fake HTTP server, or other fixture data.

### Guidance

Use `tempfile::TempDir` for isolated file system fixtures. When a fixture needs more than the
directory, wrap it in a helper struct that keeps the `TempDir` alive for the test:

```rust
struct TestState {
    _temp_dir: TempDir,
    state_dir: PathBuf,
}

impl TestState {
    fn new() -> Result<Self> { ... }
}
```

Use `wiremock::MockServer` for HTTP fixtures (see STYLE-0024), never the real Google APIs.

Use the `insta` crate for snapshot (golden) tests where output stability matters; the
`--help` snapshots live in [`tests/snapshots/`](../tests/snapshots/).

Do not commit large binary fixtures. Prefer constructing test data programmatically.

### Motivation

Temporary directories prevent tests from interfering with each other or with the real
working directory. Snapshot testing with `insta` catches unintended output regressions
without manually maintaining expected-output files.

---

## STYLE-0011: Import ordering

**Tags:** `code-style`

### Situation

Adding `use` statements to a file.

### Guidance

Group imports into three blocks separated by a blank line, in this order:

1. **Standard library** (`std`, `core`, `alloc`)
2. **External crates** (everything from `Cargo.toml` dependencies)
3. **Crate-internal** (`crate::`, `super::`, `self::`)

Within each group, let `cargo fmt` sort alphabetically.

```rust
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::drive::client::DriveClient;
use crate::utils::env::EnvSource;
```

**Enforcement note:** The rustfmt option `group_imports = "StdExternalCrate"` that codifies
this convention is still unstable. The three-group ordering is therefore a manual discipline
— `cargo fmt` will sort *within* a group but will not insert or enforce the blank-line
separators between groups. Review for this during code review.

### Motivation

Grouped imports make it easy to see at a glance what a module depends on externally versus
internally. The three-group convention is widely used in the Rust ecosystem. Alphabetical
ordering within groups is enforced by `cargo fmt`.

---

## STYLE-0012: Clippy configuration

**Tags:** `code-style`

### Situation

Configuring or overriding Clippy lints.

### Guidance

Lint configuration is centralized in `Cargo.toml` under `[lints.rust]` and `[lints.clippy]`.
The project enables `clippy::all`, `clippy::pedantic`, and `clippy::nursery` as warnings, with
specific lints allowed where they are too noisy or conflict with project conventions. See
`Cargo.toml` for the full allow-list with justification comments.

Project-specific thresholds (argument count, cognitive complexity, etc.) are configured in
`clippy.toml`. Formatting rules are documented in `rustfmt.toml`.

The only lint attributes remaining in `src/lib.rs` are `#![warn(missing_docs)]`, which is
kept there because it should only apply to the library crate, not to tests or the binary,
and `#![deny(rustdoc::broken_intra_doc_links)]`.

When suppressing a lint on a specific item, use `#[allow(clippy::...)]` with a justification
comment explaining why the suppression is necessary:

```rust
#[allow(clippy::too_many_arguments)] // Builder pattern requires all fields at construction
fn new(title: &str, description: &str, ...) -> Self { ... }
```

Do not add blanket `#[allow(...)]` at module or crate level to silence warnings. Fix the
warning or add the allow to `Cargo.toml` with a justification comment. Per-item suppression
is preferred for one-off cases; `Cargo.toml` allows are for project-wide decisions.

### Motivation

Enabling `pedantic` and `nursery` catches subtle issues that `clippy::all` misses, such as
inefficient string conversions, redundant closures, and inconsistent formatting. Centralizing
configuration in `Cargo.toml` makes the lint policy visible and auditable without searching
through source files. The allow-list documents deliberate exceptions rather than silently
suppressing noise.

---

## STYLE-0013: Unsafe policy

**Tags:** `unsafe`, `code-style`

### Situation

Considering the use of `unsafe` code.

### Guidance

This project denies `unsafe` code via `unsafe_code = "deny"` under `[lints.rust]` in
`Cargo.toml`. This lint is a hard error and applies to the entire crate.

If `unsafe` is ever required (e.g., FFI), it must be:

1. Justified in an ADR
2. Isolated in a dedicated module
3. Annotated with a `// SAFETY:` comment per Clippy's `undocumented_unsafe_blocks` lint

The one exception is the macOS `LocalAuthentication` / `SessionGetInfo` FFI in
[`src/drive/lease/authenticate/macos.rs`](../src/drive/lease/authenticate/macos.rs)
([ADR-0080](adrs/adr-0080.md)), which opts out per item with `#[allow(unsafe_code)]`.

### Motivation

gwi has almost no need for `unsafe` — it delegates low-level operations to well-audited
dependencies (`reqwest`, `tokio`, `nix`). The `deny` lint makes this a compile-time
guarantee rather than a convention. Requiring an ADR for any future exception ensures the
decision is reviewed and documented.

---

## STYLE-0014: `#[must_use]` annotation

**Tags:** `api-design`

### Situation

A public function or method returns a computed value without side effects.

### Guidance

Apply `#[must_use]` to public functions whose return value is the entire point of the call.
Discarding the result is almost certainly a bug:

```rust
#[must_use]
pub fn base_url(&self) -> &str { ... }

#[must_use]
pub fn is_system(&self) -> bool { ... }
```

**Do not apply** `#[must_use]` to:

- Functions that return `Result` — the `#[must_use]` on `Result` itself already covers this.
- Builder methods that return `&mut Self` — the builder pattern implies chaining.
- Functions with meaningful side effects (I/O, mutation) where the return value is
  supplementary.

### Motivation

`#[must_use]` turns silent logic errors (ignoring a return value) into compiler warnings.
Applying it deliberately to pure computations catches bugs at compile time without producing
false positives on side-effectful functions. This aligns with `clippy::must_use_candidate`
from the `pedantic` group.

---

## STYLE-0015: String parameter ownership

**Tags:** `api-design`

### Situation

Deciding whether a function parameter should be `&str`, `String`, or generic.

### Guidance

Use the cheapest type that satisfies the function's needs:

| The function…                          | Accept              | Example                                     |
|----------------------------------------|---------------------|---------------------------------------------|
| Only reads the string                  | `&str`              | `fn parse_since(s: &str)`                   |
| Stores the string in a struct/`Vec`    | `String`            | `fn set_title(&mut self, title: String)`    |
| Needs flexibility (public API surface) | `impl Into<String>` | `fn new(name: impl Into<String>) -> Self`   |

Prefer `&str` for internal helpers and `impl Into<String>` sparingly — only at public API
boundaries where caller ergonomics justify the generic. Avoid `impl AsRef<str>` unless you
genuinely need to accept both `String` and `&str` without conversion.

For return types, prefer `&str` when returning a reference to owned data, and `String` when
returning a newly constructed value. Avoid `Cow<'_, str>` unless profiling shows the
borrow-or-own flexibility is needed.

```rust
// Good — borrows for read-only access
pub fn name(&self) -> &str {
    &self.name
}

// Good — takes ownership because it stores the value
pub fn with_title(mut self, title: String) -> Self {
    self.title = title;
    self
}

// Good — constructs a new string
pub fn format_summary(&self) -> String {
    format!("{}: {}", self.id, self.name)
}
```

### Motivation

Accepting `&str` avoids unnecessary allocations on the caller side. Taking `String` when
ownership is needed makes the transfer explicit and avoids hidden `.to_string()` calls
inside the function. The `impl Into<String>` pattern is convenient for public APIs but adds
monomorphisation cost, so it should be used judiciously.

---

## STYLE-0016: Named constants

**Tags:** `code-style`, `naming`

### Situation

Using a numeric or string literal whose meaning is not obvious from surrounding context.

### Guidance

Extract **magic literals** into named constants or `const` items. A literal is "magic" when its
purpose is not self-evident at the usage site:

```rust
// Bad — what does 8 mean?
let short = &file_id[..8];

// Good — the name documents the intent
const SHORT_ID_LEN: usize = 8;
let short = &file_id[..SHORT_ID_LEN];
```

```rust
// Bad — why 3?
if auth_attempts > 3 {
    bail!("Too many authentication attempts");
}

// Good
const MAX_AUTH_ATTEMPTS: u32 = 3;
if auth_attempts > MAX_AUTH_ATTEMPTS {
    bail!("Too many authentication attempts");
}
```

Literals that do **not** need extraction:

- **Structural zeros and ones** — `Vec::with_capacity(1)`, `index + 1`, `slice[0]`.
- **Format strings** — `format!("{}: {}", key, value)`.
- **Known-safe constructor arguments** — `FixedOffset::east_opt(0)` (covered by STYLE-0003).
- **Test assertions** — `assert_eq!(result.len(), 3)` where the value is local to the test.

Place constants at the narrowest useful scope: module-level `const` if used across functions in
the same module, crate-level if shared across modules, or function-local `const` if truly local.

### Motivation

Named constants make the code self-documenting and provide a single point of change when a value
needs updating. Searching for `SHORT_ID_LEN` finds every usage; searching for `8` returns
hundreds of false positives. The exceptions prevent over-extraction of trivially obvious values.

---

## STYLE-0017: Function length

**Tags:** `code-style`

### Situation

Writing or reviewing a function that is growing long.

### Guidance

Keep functions **under ~50 lines** of logic (excluding doc comments, blank lines, and closing
braces). When a function exceeds this guideline, look for opportunities to extract coherent
sub-operations into well-named helper functions.

Common extraction targets:

- **Setup / teardown** — opening resources, building configuration structs.
- **Distinct phases** — validation, transformation, output formatting.
- **Repeated patterns** — similar blocks that differ only in parameters.
- **Nested closures or callbacks** — especially retry handlers and stream callbacks.

```rust
// Before — 120-line execute() mixing validation, API calls, file I/O, and display
fn execute(&self) -> Result<()> {
    // ... 120 lines ...
}

// After — orchestrator delegates to focused helpers
fn execute(&self) -> Result<()> {
    let account = self.resolve_account()?;
    let files = self.fetch_files(&account)?;
    let report = self.build_report(&files)?;
    self.write_output(&report)?;
    Ok(())
}
```

This is a **guideline, not a hard limit**. A 60-line function that reads linearly may be clearer
than three 20-line functions with non-obvious data flow. Use judgement — the goal is readability,
not a line count.

### Motivation

Long functions are harder to name, test, and review. Extracting sub-operations gives each piece
a name that serves as documentation and makes the top-level flow scannable. The ~50-line
heuristic is a common industry threshold (Clean Code, Effective Rust) that balances granularity
against fragmentation.

---

## STYLE-0018: Silent error suppression

**Tags:** `error-handling`

### Situation

Handling a `Result` or `Option` where the error/`None` case is intentionally ignored.

### Guidance

**Never silently discard an error that could indicate a real problem.** Three patterns to watch
for:

1. **`let _ = fallible_call();`** — If the operation can meaningfully fail, at least log the
   error at `debug!` or `warn!` level. If the failure is truly inconsequential (best-effort
   cleanup), add a comment explaining why:

   ```rust
   // Bad — caller has no idea the cleanup failed
   let _ = fs::remove_file(&partial_download);

   // Good — intent is documented, failure is logged
   // Best-effort cleanup; the partial file may already be gone.
   if let Err(e) = fs::remove_file(&partial_download) {
       tracing::debug!("Removing the partial download failed: {e}");
   }
   ```

2. **`if let Ok(x) = ... { use(x) }`** with no `else` — returning a silent default on parse
   or I/O failure hides broken configuration files from the user:

   ```rust
   // Bad — silently returns no rules on a malformed file
   if let Ok(content) = fs::read_to_string(&path) {
       if let Ok(config) = serde_yaml::from_str(&content) {
           return config.rules;
       }
   }
   Vec::new()

   // Good — warns so the user knows their file was ignored
   match fs::read_to_string(&path) {
       Ok(content) => match serde_yaml::from_str(&content) {
           Ok(config) => return config.rules,
           Err(e) => tracing::warn!("Ignoring {}: {e}", path.display()),
       },
       Err(e) if e.kind() != io::ErrorKind::NotFound => {
           tracing::warn!("Cannot read {}: {e}", path.display());
       }
       _ => {} // File not found is expected in the fallback chain
   }
   ```

3. **`.unwrap_or_default()` on non-trivial results** — acceptable for genuinely optional data,
   but not as a blanket substitute for error handling on operations that should succeed.

**Acceptable silent discards:**

- Closing a file or flushing a logger during shutdown.
- Sending on a channel where the receiver may have been dropped.
- Test cleanup in `Drop` implementations.

### Motivation

Silent error suppression is one of the hardest bugs to diagnose because nothing visibly fails —
the program simply produces wrong results or missing data. Logging at `debug!` or `warn!` level
costs nothing on the success path and provides a trail when something goes wrong. The explicit
comment requirement for `let _ =` forces the author to justify the suppression at write time,
which often reveals that the error should not be ignored after all.

---

## STYLE-0019: Type-safe variant selection

**Tags:** `api-design`, `code-style`

### Situation

Routing behaviour based on a value that comes from a fixed, known set of alternatives (e.g.,
output format, account kind, permission level).

### Guidance

Model the set of alternatives as an **enum** and match on it. Do not use string comparisons to
branch on known variants:

```rust
// Bad — brittle, easy to typo, no exhaustiveness checking
let rendered = if format.to_lowercase().contains("json") {
    to_json(&rows)?
} else {
    to_table(&rows)
};

// Good — the compiler enforces every variant is handled
enum OutputFormat {
    Table,
    Json,
    Yaml,
}

fn render(rows: &[Row], format: &OutputFormat) -> Result<String> {
    match format {
        OutputFormat::Table => Ok(to_table(rows)),
        OutputFormat::Json => to_json(rows),
        OutputFormat::Yaml => to_yaml(rows),
    }
}
```

**Parse once, branch on the enum everywhere else.** The string-to-enum conversion should happen
at the boundary (CLI parsing, config loading, environment variable reading). All downstream code
receives the enum and uses `match`, which the compiler checks for exhaustiveness.

This applies to any situation where the set of values is known at compile time — not just
output formats ([`OutputFormat`](../src/cli/format.rs) is the real one, bound to `-o/--output`
by `clap`'s `ValueEnum`). Log levels, feature flags, and similar categories all benefit from
the same pattern.

### Motivation

String-based dispatching defeats Rust's exhaustiveness checking. When a new variant is added,
the compiler cannot tell you which `if` chains need updating — you discover missed branches at
runtime. An enum makes invalid states unrepresentable and turns forgotten branches into compile
errors. The "parse at the boundary" pattern also eliminates repeated `.to_lowercase().contains()`
calls scattered across the codebase.

---

## STYLE-0020: Single-purpose commits

**Tags:** `commits`

### Situation

Preparing a set of changes that involves refactoring, new functionality, or bug fixes.

### Guidance

Each commit should do **one kind of work**. Keep refactoring commits separate from
implementation commits, and both separate from bug-fix commits.

If a refactoring would make a subsequent implementation or fix cleaner, land the refactoring
as an **earlier** commit so that:

1. The refactoring can be reviewed on its own terms (no behaviour change expected).
2. The implementation commit starts from a cleaner baseline and is easier to understand.
3. Either commit can be reverted independently if needed.

```
# Good — reviewable, bisectable, revertible
git log --oneline
a1b2c3  refactor(cli): extract shared account resolver
d4e5f6  feat(cli): add --json output to the search command

# Bad — mixed intent, hard to review or revert half of it
git log --oneline
f7g8h9  feat(cli): add --json output and refactor account resolver
```

**Acceptable exceptions:**

- Trivial renames or import cleanups that are a natural by-product of the implementation
  (a few lines, not a standalone refactoring effort).
- Prototype or spike branches where commit hygiene is deferred to a squash before merge.

### Motivation

Single-purpose commits make `git bisect` reliable, code review focused, and reverts
surgical. When refactoring is interleaved with behaviour changes, reviewers cannot tell
whether a difference is a deliberate new behaviour or a mechanical restructuring — so they
must verify every line as if it were new logic. Separating the two cuts review effort
roughly in half.

---

## STYLE-0021: Module cohesion

**Tags:** `module-organization`

### Situation

A source file is accumulating types, functions, or `impl` blocks that serve unrelated
purposes.

### Guidance

Each module should have a **single, nameable responsibility**. When you find it hard to
describe what a module does without using "and," it likely contains unrelated code that
would be clearer in separate submodules.

**Signals that a module should be split:**

- It contains multiple independent command or handler types that share little or no private
  state (e.g., `SearchCommand`, `TrashCommand`, and `UploadCommand` in one file).
- Unrelated sections require scanning past hundreds of lines to find the piece you need.
- Changes to one logical area routinely cause merge conflicts with work in another area of
  the same file.
- You struggle to name the file — broad names like `commands.rs` or `helpers.rs` suggest
  mixed responsibilities.

**What is *not* a reason to split:**

- Line count alone. A 400-line module with a single cohesive type and its helpers is fine.
- A few shared utility functions that genuinely serve every type in the module.

When splitting, apply the layout from STYLE-0004 and extract each distinct responsibility
into its own submodule:

```
# Before — one file with several unrelated command types
src/cli/drive.rs        # thousands of lines, every command + helpers

# After — each command owns its module, shared code is explicit
src/cli/
├── drive.rs            # declares submodules, shared types
└── drive/
    ├── search.rs       # SearchCommand
    ├── trash.rs        # TrashCommand
    ├── upload.rs       # UploadCommand
    └── helpers.rs      # shared client construction
```

### Motivation

A module that mixes unrelated responsibilities is hard to navigate, produces noisy diffs,
and invites merge conflicts between independent work streams. Splitting by responsibility
makes each file's purpose obvious from its name, keeps diffs focused on the change at hand,
and lets reviewers evaluate one concern at a time. The emphasis on cohesion rather than a
rigid line limit avoids unnecessary churn on files that are large but focused, while still
flagging files that are large *because* they mix concerns.

---

## STYLE-0022: ADR format

**Tags:** `adrs`

### Situation

Writing a new Architecture Decision Record or reviewing an existing one.

### Guidance

Every ADR must use exactly the structure prescribed by
[ADR-0000](adrs/adr-0000.md): **Title**, **Status**, **Context**, **Decision**,
**Consequences**. Do not add extra top-level sections (e.g., "Recommendations",
"Alternatives", "References"). Content that might seem like a separate section should be
incorporated into the appropriate prescribed section — alternatives belong in Context,
recommendations belong in Decision or Consequences.

The Decision section must be stated in active voice ("We will ..."). The Consequences
section should cover positive, negative, and neutral outcomes.

### Motivation

A consistent structure makes ADRs scannable and sets clear expectations for both authors
and reviewers. Extra sections blur the boundary between architectural decisions and
operational guidance (which belongs in the style guide) or implementation detail (which
belongs in code comments or docs).

---

## STYLE-0023: Validate commit messages with omni-dev after creation

**Tags:** `commits`

### Situation

After creating one or more commits and before pushing or opening a pull request.

### Guidance

After every `git commit`, and before pushing, run
`omni-dev git commit message lint origin/main..HEAD` to validate the messages against the
guidelines in [`.omni-dev/commit-guidelines.md`](../.omni-dev/commit-guidelines.md); it is
deterministic and needs no API key. CI runs the same check
([`commit-lint.yml`](../.github/workflows/commit-lint.yml)), so a message that fails here
fails the pull request. Fix a failing message with `omni-dev git commit message amend`.

**Constraints to observe:**

- The target commit must be at the branch tip with **no merge commit above it**. If a
  merge commit is present the amend step will fail — work on a branch before merging.
- The amendments file requires the **exact 40-character SHA** from the commit output.
  An abbreviated hash silently skips the amendment or errors.
- Do **not** include a `Co-Authored-By` footer unless a human co-author contributed.
  AI tool attribution footers must not be added to commit messages.

### Motivation

Running the lint after commit creation catches scope, casing, and footer
violations before they reach the remote, avoiding the costly reset-and-redo cycle
required to rewrite history once a commit has been merged to `main`.

---

## STYLE-0024: Wiremock tests for Gmail and Drive client methods

**Tags:** `testing`, `api-design`

### Situation

Adding a new public method that calls the Gmail or Drive REST API: the API façades
(`LabelsApi` in [`src/gmail/labels_api.rs`](../src/gmail/labels_api.rs), the `*_api.rs` modules
beside it, and their Drive counterparts under [`src/drive/`](../src/drive/)) and the clients
in [`src/gmail/client.rs`](../src/gmail/client.rs) and
[`src/drive/client.rs`](../src/drive/client.rs).

### Guidance

Every new public method that sends a request must have corresponding `#[tokio::test]`
tests using `wiremock::MockServer`. At minimum, cover three cases:

1. **Success** — mock the expected HTTP method and path, return a valid response, and
   assert on the parsed result fields.
2. **Empty / edge case** — return a valid but minimal response (e.g., empty list, zero
   count) and assert the method handles it gracefully.
3. **API error** — return a non-success status code (e.g., 404, 403) and assert the
   error is propagated with the status code in the message.

Follow the existing test pattern in `labels_api.rs`:

```rust
#[tokio::test]
async fn get_builds_correct_url_and_parses_a_single_label() {
    let server = wiremock::MockServer::start().await;
    let client = client_with_bootstrapped_token(&server).await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/gmail/v1/users/me/labels/Label_1"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "Label_1",
                "name": "Finance",
                "type": "user",
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let label = LabelsApi::new(&client).get("Label_1").await.unwrap();
    assert_eq!(label.name, "Finance");
}
```

`client_with_bootstrapped_token` mounts the OAuth token endpoint on the same mock server, so
the test needs no real credentials and one server per test.

### Motivation

Client methods are the project's primary integration boundary with the Google REST
APIs. Wiremock tests verify request construction (method, path, query params, body) and
response parsing without hitting a live API. Skipping these tests leaves the entire
HTTP layer uncovered, which CI coverage checks will flag as a patch coverage gap.

---

## STYLE-0025: Testable CLI execute methods

**Tags:** `testing`, `module-organization`

### Situation

Adding or modifying a CLI command in `src/cli/drive/` or `src/cli/gmail/`.

### Guidance

The parent command builds the client from credentials it reads from the environment and
settings (`create_client` / `create_client_for` in
[`src/cli/drive/helpers.rs`](../src/cli/drive/helpers.rs) and
[`src/cli/gmail/helpers.rs`](../src/cli/gmail/helpers.rs)), so a subcommand's `execute` is
unreachable in unit tests with a real client. When an `execute` method contains **non-trivial
logic**, extract that logic into a standalone `run_*` function that accepts a `&DriveClient` or
`&GmailClient` (or the relevant API wrapper) so it can be tested with wiremock.

**Extract when** the `execute` body contains any of:

- Multi-step orchestration (e.g., fetch → resolve → mutate → confirm).
- Branching or validation on user input (e.g., resolving a folder by name or ID,
  parsing and validating file ids, confirmation prompts).
- Logic that combines results from multiple API calls.

```rust
impl CreateCommand {
    pub async fn execute(self, client: &DriveClient) -> Result<()> {
        // ... build the options from the flags, load the permission rules ...
        run_create(client, &opts, &rules, &self.output).await
    }
}

async fn run_create(
    client: &DriveClient, opts: &CreateOptions, rules: &[FolderPermissionRule], output: &OutputFormat,
) -> Result<()> {
    let outcome = create::create(client, opts, rules).await;
    // ... emit the outcome in the requested format ...
}
```

(See [`src/cli/drive/create.rs`](../src/cli/drive/create.rs) and
[`src/cli/drive/dedupe.rs`](../src/cli/drive/dedupe.rs).)

Write tests for `run_*` functions covering the success path, structured output formats
(JSON/YAML), and API error propagation.

**Do not extract when** `execute` is a trivial pipeline — a single API call fed directly
into `output_as` / print with no branching or validation:

```rust
impl ListCommand {
    pub async fn execute(self, client: &GmailClient) -> Result<()> {
        let result = LabelsApi::new(client).list().await?;
        if output_as(&result, &self.output)? {
            return Ok(());
        }
        print_labels(&result);
        Ok(())
    }
}
```

Here the client method itself should have wiremock tests (per STYLE-0024), and extraction
would add indirection without catching additional bugs. Inline is fine. (The real
`gmail label list` still has a `run_list`, because it also owns the table rendering and its
empty-result message; extraction is allowed when it buys a test, just not required.)

### Motivation

The goal is **testability of logic that can break**, not mechanical conformance. Extracting
a trivial pipeline adds a function boundary and a signature to maintain without delivering
new test coverage beyond what STYLE-0024 client tests already provide. Reserving extraction
for commands with real orchestration or validation keeps the codebase lean while ensuring
the code most likely to harbour bugs is covered.

## STYLE-0026: MCP tool authoring conventions

**Tags:** `api-design`, `module-organization`, `testing`

### Situation

Adding or modifying MCP tools or supporting types under `src/mcp/`. gwi serves tools only,
no resources (`server_info_advertises_only_the_tools_capability`).

### Guidance

1. **Parameter structs.** Every tool defines its input as a dedicated
   `#[derive(Debug, Deserialize, schemars::JsonSchema)]` struct with a name
   ending in `Params` (e.g. `DriveDocsReplaceParams`). All fields get a doc
   comment — it flows through to the tool's JSON schema and is what the
   assistant sees. Optional fields use `#[serde(default)]` and `Option<T>`;
   never `Default::default()` in the handler body.

2. **One tool router per module.** Group related tools in their own submodule
   and expose the router via `#[tool_router(router = name_tool_router, vis = "pub")]`
   (see [src/mcp/drive_tools.rs](../src/mcp/drive_tools.rs)). `GwiServer::new`
   in [src/mcp/server.rs](../src/mcp/server.rs) combines all routers — add a new
   module there rather than cramming tools into an existing router.

3. **Error mapping.** Inside tool handlers, bubble `anyhow::Error` out via
   the shared [`tool_error`](../src/mcp/error.rs) helper so the full error
   chain reaches the client. Do **not** build `McpError` values by hand with
   bespoke messages — go through `tool_error` so the format stays consistent
   across tools.

4. **Blocking work belongs in `spawn_blocking`.** Tools that call into
   synchronous business logic (e.g. local file reads and writes, as
   `drive_docs_append` does with `append_text`) must wrap the call
   in `tokio::task::spawn_blocking` — the MCP transport loop is async, and
   blocking it stalls every in-flight request.

5. **Output format.** YAML for structured results (matches the CLI's
   `-o yaml`, so the tool and the command describe the same data), plain text
   for rendered prose such as document content. Return large payloads through
   [`build_truncated_result`](../src/mcp/truncate.rs) so a response over the
   size cap is cut at a UTF-8 boundary and flagged as truncated instead of
   flooding the client's context. Write tools return complete tagged YAML and
   set `is_error` on refusals.

6. **Testing.** Tools need at least:
   - A library-level unit test covering the success path with a fabricated
     input (a wiremock server through the `run_*` function the handler
     calls, or a temp dir).
   - An integration test in [`tests/mcp_test.rs`](../tests/mcp_test.rs) that
     spins up `GwiServer` on an in-memory duplex and exercises the MCP
     protocol round-trip (list + call). Add the tool to the pinned tool lists
     there so a tool cannot appear or vanish unnoticed.

### Motivation

The MCP surface is consumed by non-human clients that can only see what the
schema and error messages tell them. Uniform parameter structs make the
schema predictable; shared error mapping keeps diagnostics legible across
tools; router splitting keeps modules small and testable; the paired
unit+integration test requirement means a regression in protocol wiring is
caught without requiring a live MCP client to reproduce.

---

## STYLE-0027: Plan-file status header and ADR cross-links

**Tags:** `documentation`, `adrs`

### Situation

Adding or substantially editing a file in [`docs/plan/`](plan/).

### Guidance

1. **Status header.** Immediately after the `# Title` heading, add a `**Status:** …` line using one of these four canonical tags:

   | Tag             | Meaning                                                                                          |
   |-----------------|--------------------------------------------------------------------------------------------------|
   | `Built`         | The design has shipped. The doc may still be useful as a reference but is not a roadmap.         |
   | `In Progress`   | Some phases shipped, others ongoing. Specify which phase is current.                             |
   | `Aspirational`  | Describes intent that is not yet started or has been superseded by a different approach.         |
   | `Historical`    | Written for context that no longer matches current architecture; kept for institutional memory.  |

   A short qualifier after an em-dash (`— canonical reference`, `— Phase 3 not started`) is encouraged when it adds signal.

2. **ADR cross-links.** When one or more ADRs describe the same decisions, add an `**ADRs:**` line immediately after the Status line, listing each ADR as a relative link separated by ` · ` (middle dot, surrounded by spaces). Example:

   ```markdown
   **ADRs:** [ADR-0000](../adrs/adr-0000.md) · [ADR-0001](../adrs/adr-0001.md)
   ```

3. **When to retire.** Once a plan's decisions are captured in one or more ADRs, change its status to `Built` (with ADR cross-links) or `Historical` rather than deleting it — preserving the doc keeps prior reasoning discoverable.

4. **When to promote.** When a plan's high-level decisions stabilise, copy the decision and its rationale into a new ADR (see [STYLE-0022](#style-0022-adr-format)) and update the plan's status to `Built` with a cross-link.

### Motivation

A plan directory that mixes shipped, in-progress, and superseded content with no signalling forces every new contributor to read every file and cross-check the codebase before they can act on it. A one-line status header costs the author nothing and gives the reader an immediate orientation; ADR cross-links make the canonical decision discoverable.

## STYLE-0028: Inject the environment, don't mutate it in tests

**Tags:** `testing`, `module-organization`

### Situation

Writing code that reads an environment variable, or writing a test for code
whose behaviour depends on the environment (`HOME`, `XDG_CONFIG_HOME`,
`GWI_*`, `GMAIL_*`, `DRIVE_*`, credential vars, …).

### Guidance

**Read the environment only at a thin boundary wrapper; put the logic in an
inner seam that takes the resolved input as a value.** Then tests exercise the
inner seam with a constructed value and have no reason to mutate the
process-global environment. Pick the seam by what is read:

1. **Resolved domain value** — incidental config. Provide a `*_from(value)`
   constructor alongside the env-resolving entry point. (e.g.
   [`create_client_from`](../src/cli/drive/helpers.rs) and its
   [Gmail twin](../src/cli/gmail/helpers.rs),
   [`DriveClient::from_credentials`](../src/drive/client.rs).)

   ```rust
   pub fn create_client_for(account: Option<&str>) -> Result<DriveClient> {
       create_client_from(auth::load_credentials_for(account)?)   // prod: resolve env → value
   }
   pub fn create_client_from(credentials: DriveCredentials) -> Result<DriveClient> { /* … */ }
   ```

2. **`std::env::var` parsing boundary** — "given these vars, what do we do?".
   Take `&impl EnvSource` (see [`crate::utils::env`](../src/utils/env.rs)); the
   prod wrapper passes `&SystemEnv`, tests pass a `MapEnv`
   ([`crate::test_support::env`](../src/test_support.rs)).

   ```rust
   pub fn from_credentials(credentials: &DriveCredentials) -> Result<Self> {
       Self::from_credentials_with(&SystemEnv, credentials)    // thin wrapper
   }
   pub(crate) fn from_credentials_with(env: &impl EnvSource, credentials: &DriveCredentials) -> Result<Self> { /* … */ }
   ```

   ([`request_log.rs`](../src/request_log.rs) has more: `disabled_with`, `log_file_path_with`.)

3. **`dirs::home_dir()` / `dirs::config_dir()`** — these read `HOME` /
   `XDG_CONFIG_HOME` *inside* the `dirs` crate, where `EnvSource` can't reach.
   Thread the resolved base directory as a parameter (prod default =
   `dirs::home_dir()`). For a subprocess that needs `HOME`, set it scoped on
   the `Command` (`.env("HOME", …)`), never on the process.

**gwi convention.** Prefer an injected seam (`default_local_state_path_for(os)`,
`import_client_credentials_to(…, home, …)`, a `*_with(&MapEnv)` function) so the
test reads no `HOME` and mutates no variable. Some tests cannot: they drive
product code that reads `std::env` itself (credentials, the `*_API_URL` hosts
pointed at a mock server), or they exist to hold `HOME` still
(`drive::chrome_profile`). Those tests take `EnvGuard`
(`crate::gmail::test_support` / `crate::drive::test_support`) and are the only
tests that call `std::env::set_var` / `remove_var`:

- **One lock.** Both guards hold `crate::test_support::HOME_ENV_MUTEX`. A new
  module aliases that static and **never declares its own env lock** (a
  `Mutex`, a `serial_test` attribute, …): a per-module lock excludes only that
  module's tests, so two modules still race
  on the same global (omni-dev #950, #1465). Anything that sets `HOME`, or a
  variable resolved through it, must hold the guard.
- **Mutate only under the guard.** Take `EnvGuard::take()` before the first
  `set_var` / `remove_var`. Drop restores only the keys in its snapshot, so
  add a new variable to that list (`EnvGuard::keys()`) before a test mutates
  it. The Drive guard also snapshots and clears `GWI_DRIVE_LEASE_*`,
  because default-policy tests resolve them through settings. A new secret's
  `_FILE` / `_COMMAND` companions are picked up from `SECRET_ENV_VARS`.
- **A test that needs a variable unset takes the guard.** The credential,
  account, profile and endpoint variables (`GMAIL_*`, `DRIVE_*` with their
  `_FILE` / `_COMMAND` companions, `GWI_PROFILE`, `GWI_*_ACCOUNT`, the
  `*_API_URL` overrides) are exported by guarded tests, and by a developer's
  shell. A test whose result depends on one being **unset** (a default API
  host, "no profile selected", "not configured") takes the guard and calls
  `clear_credentials()`, even for a single read (#62). A spawned `gwi`
  removes them with `tests/common`'s `scrub_ambient_env`.
- **Logging uses a local seam.** Unit-test logging ignores ambient
  `GWI_LOG_*` overrides (including rotation and header/body opt-ins). To observe
  request records, use `crate::test_support::RequestLogGuard`; audit records use
  `AuditLogGuard`. Both route only the current thread, so spawned tasks need
  their own route. Test environment parsing with `MapEnv` and the `*_with`
  functions. Mutating an env var under the shared mutex cannot protect ordinary
  logging calls on other threads. Integration tests exercise the production
  environment boundary through `Command::env` and scrub ambient logging,
  HTTP/secret-command limits and lease policy with `tests/common`.
- **Timing-sensitive HTTP tests inject a transport.** A mock response deliberately
  held in flight must outlive the observation window independently of
  `GWI_HTTP_*` exports. Construct a transport with explicit timeouts and pass it
  through the client seam (`GmailClient::with_http_client`), instead of changing
  process timeouts. Secret-command limit tests inject `MapEnv` or `Limits`.
- **Enforcement is partial.**
  `every_gmail_and_drive_env_mutation_holds_the_env_guard` fails a function
  that mutates one of the credential or endpoint variables in its
  `GUARDED_KEYS` without `EnvGuard::take()`. That list omits `GWI_PROFILE`
  (`--profile` sets it in production), and `HOME` has no such check.
- **When a `HOME`-derived read needs the guard.** When the test reads one
  **more than once**, compares it, or relies on it staying put, because another
  test repointing `HOME` between the reads makes them disagree (#14, #15, #17).
  "Reads a `HOME`-derived value" includes everything that reaches `dirs::*`:
  `Settings::get_settings_path`, `request_log::gwi_state_subpath` and so its
  callers (the request and audit logs, `lease::ledger::ledger_path`,
  `default_backup_dir`), `resolve_config_file`, `guard_output_dir`. The guard
  excludes only tests that also take the mutex, so anything that sets `HOME`
  must hold it too. A single read whose use holds for any `HOME`
  (`path.ends_with("Local State")`) needs no guard.

Everywhere else, do not call `std::env::set_var` / `remove_var` in a test.

#### Default state-directory paths must be safe in tests

New code that resolves a default writable runtime path through
`request_log::gwi_state_subpath`, or directly through `dirs::state_dir()` /
`dirs::data_dir()`, must provide either an injected path parameter that tests
use or a `#[cfg(test)]` scratch fallback. This includes incidental writes
from production code exercised by tests, even when the test does not inspect
the file. Holding `EnvGuard` prevents environment races; it does not redirect
writes away from the developer's state directory.

Follow `default_audit_file_path` and `default_log_file_path` in
[`src/request_log.rs`](../src/request_log.rs): the `#[cfg(not(test))]` variant
resolves the production default, while the `#[cfg(test)]` variant returns a
path in `test_scratch_dir`. Keep the scratch directory alive for every caller
that may write to it. Tests that inspect file contents use their own injected
temporary path rather than a fallback shared by parallel tests. Preserve
explicit overrides; the audit log's `TEST_AUDIT_ROUTE` / `AuditLogGuard`
pattern lets individual tests opt into their own output path.

Add a regression test with overrides absent that proves the test default is
outside the real state directory, not merely different from one filename.
See `log_file_path_never_resolves_to_the_real_machine_default_when_unset` in
the same file; also check stability across calls when the fallback is shared.
For an injected path, exercise the writable flow with a temporary path and
verify its output stays there. Follow the guard requirements above whenever
the test relies on a `HOME`-derived value staying fixed across reads.

Verify the suite with an **empty `HOME`** as well: build test binaries with
the real `HOME` (`cargo test --lib --bins --tests --no-run --message-format=json`),
then run the reported test executables directly with a fresh temporary `HOME`
for each binary. Clear inherited `XDG_STATE_HOME`, `XDG_DATA_HOME`,
`XDG_CONFIG_HOME` and `XDG_CACHE_HOME` for those subprocesses so they cannot
route writes outside the probe. Set `INSTA_WORKSPACE_ROOT` to the checkout
so snapshot discovery does not invoke Cargo through rustup under the empty
`HOME`. After each run, `find "$probe_home" -mindepth 1` must print nothing,
including when the binary fails; repeat with `--features mcp`. Cargo itself
needs its normal home/toolchain configuration, so do not run the build step
under the empty `HOME`. The CI guard and local runner are tracked in
[#118](https://github.com/rust-works/gwi/issues/118).

Integration tests use the normal library / CLI build, where `#[cfg(test)]`
fallbacks do not apply. Inject paths or scope `HOME` and XDG directory
overrides on the spawned `Command` to a temporary fixture. The empty-`HOME`
probe covers both unit and integration binaries, but detects only entries
remaining under the supplied home after execution, not transient writes or
writes to unrelated absolute paths.

### Motivation

The process environment is a single shared mutable global. Injecting the
resolved value removes the hazard entirely: env-dependent tests become pure,
order-independent, lock-free, and fully parallel. Where a test has to mutate it
anyway, every mutation must share the one lock, because per-module mutexes
provide **no** mutual exclusion across modules.

`set_var` / `remove_var` are safe in the crate's edition (`edition = "2021"`)
and `unsafe` in Rust 2024. The crate also sets `unsafe_code = "deny"`, so
moving to 2024 will not compile until every call site is either wrapped in
`unsafe` (with `unsafe_code` allowed there) or injected away. That is not only
the two `test_support` modules: the tests that mutate under the guard, and the
product code that does (`propagate_profile_flag`, `ScopedEnvVar`), all count.
Each new direct `set_var` adds to that bill, which is why a seam is the first
choice.

Default paths can turn otherwise isolated tests into real data writers. The
audit log needed a scratch fallback first; the request log later appended
about 8,000 rows per suite run to the developer's `log.jsonl`
([#61](https://github.com/rust-works/gwi/issues/61),
[PR #86](https://github.com/rust-works/gwi/pull/86)). A safe default plus the
empty-`HOME` check prevents the same mistake in the next runtime state file.

## STYLE-0029: MCP tool & parameter description checklist

**Tags:** `documentation`, `api-design`

### Situation

Writing or revising any `#[tool(description = "…")]` text or any doc comment on
a `*Params` struct field under `src/mcp/`. These strings are the **only** thing
an AI agent reads to decide how to call a tool: field doc comments flow through
`schemars::JsonSchema` into each field's JSON-schema `description`, and
`description = "…"` sets the tool-level text. [STYLE-0026](#style-0026-mcp-tool-authoring-conventions)
covers the *wiring* (struct shape, router, tests); this rule covers the *prose
quality* the agent actually reads.

### Guidance

Every tool description and parameter doc comment must satisfy this checklist.
The reference exemplars are the write tools in
[`src/mcp/drive_write_tools.rs`](../src/mcp/drive_write_tools.rs): `DriveDocsReplaceParams`
(`search`/`replace` with concrete `draft` → `final` values and the empty-deletes case) and the
`drive_docs_replace` tool description.

**Tool-level `description`:**

1. **One-line "what it does"** as the first sentence — a single, skimmable
   summary an agent reads before the params.
2. **CLI cross-reference, both directions.** The tool description names its
   equivalent subcommand: ``Mirrors `gwi <subcommand>`.`` The clap
   subcommand's doc comment carries the reverse, ending with
   ``(mirrors the `<tool_name>` MCP tool)`` (see the `Auth`, `Account` and `Search`
   variants in [src/cli/gmail.rs](../src/cli/gmail.rs)). The
   two must stay in lock-step. A tool with no CLI equivalent says so explicitly.
3. **"When to use vs `<sibling>`"** wherever two tools overlap or could be
   confused (e.g. `drive_docs_replace` vs `drive_docs_append`; `drive_sheets_append` vs
   `drive_sheets_clear`). One sentence pointing at the sibling and when to prefer it.
4. **A concrete example in the tool text**, not only on fields — a real id, a
   real enum value, or a one-line call shape (``Example: document_id, search:"draft",
   replace:"final", dry_run:true``). Surface the single most error-prone value at
   the tool level the agent skims first.
5. **Mutating/destructive affordances.** State any `dry_run`, lease, or
   preflight behaviour and its default (e.g. "dry_run defaults to false. Dry-run
   first: previews need no lease. Real writes require an operator allow rule and
   a lease").

**Per-parameter doc comment:**

6. **A concrete example value** — ``e.g. `1a2B3c4D` ``, ``e.g. `2025-01-31` ``,
   ``e.g. `draft` ``.
7. **Allowed values for enums / closed sets** spelled out
   (``one of `metadata`, `content` ``) — don't make the agent guess.
8. **Expected wire format** — Gmail search syntax vs a Drive query, a Docs
   document id from the `/d/<ID>/` part of a URL, an A1 range (`A1:B2`),
   `YYYY-MM-DD`, an RFC 3339 timestamp.
9. **Directional / order-dependent semantics spelled out** — never "from" /
   "to" or "old" / "new" alone. Name which end is which, with an example:
   *"Literal text to find, e.g. draft; not a regular expression"* and *"Replacement
   text, e.g. final. Empty deletes matches."* An inverted pair of arguments
   is the failure this item exists to prevent.
10. **Required vs optional.** Optional fields use `#[serde(default)]` +
    `Option<T>` (per STYLE-0026) and the doc comment states the default
    behaviour when the field is omitted.

Keep the MCP mentions in [docs/gmail.md](gmail.md) (its "MCP equivalent(s)" sections) and
[docs/drive.md](drive.md) (e.g. "Also available as the `drive_docs_info` MCP tool"), and the MCP
section of the [README](../README.md#mcp-server), in sync when a tool's purpose or CLI mapping
changes, and review the `--help`
snapshots in [`tests/snapshots/`](../tests/snapshots/) with `cargo insta review`
whenever the reverse-reference edits change CLI `--help` text.

The tool and top-level parameter description floors of this checklist are enforced mechanically:
`list_tools_advertises_exactly_the_gmail_and_drive_tools` in
[tests/mcp_test.rs](../tests/mcp_test.rs) fails if any advertised tool has an empty
description or any top-level parameter has a missing, non-string, empty or whitespace-only
description (item 6's floor). Concrete examples and the other prose-quality items still
need review.

### Motivation

An agent's success rate is bounded by how unambiguous these strings are. A
vague description produces wrong-but-silent calls — a search and replacement
swapped, a Gmail query where a Drive query was required, an append where a
replace was meant — each costing a recovery round-trip or quietly corrupting
data. This checklist holds the whole `src/mcp/` surface to one standard rather
than letting it drift tool-by-tool.

---

## STYLE-0030: Secret environment variables go through the secret resolver

**Tags:** `module-organization`, `api-design`, `testing`

### Situation

Reading a credential — an API key, token, client secret, private key — from the
environment or from settings.json's `env` map, or adding a new one.

### Guidance

Register the variable in `SECRET_ENV_VARS`
([`src/utils/secret_env.rs`](../src/utils/secret_env.rs)) and read it **only**
through `secret_var` / `secret_var_any` (or `secret_var_is_set` for a
presence-only status flag). Never pass it to `EnvSource::var`, `var_any`,
`non_empty_var`, `Settings::get_env_var` or `std::env::var`.

```rust
// Good: accepts DRIVE_REFRESH_TOKEN or DRIVE_REFRESH_TOKEN_FILE, returns a Secret.
let token = secret_var(env, DRIVE_REFRESH_TOKEN)?.ok_or(DriveError::CredentialsNotFound)?;

// Bad: no _FILE support, and a plain String.
let token = env.var(DRIVE_REFRESH_TOKEN).ok_or(DriveError::CredentialsNotFound)?;
```

The resolver gives every secret a `<NAME>_FILE` companion with one set of rules
(absolute path, yours and owner-only or root's and read-only to others, one trailing newline
trimmed, two-set is an error per layer) — see
[ADR-0089](https://github.com/rust-works/omni-dev/blob/main/docs/adrs/adr-0089.md) — and a
`<NAME>_COMMAND` companion that runs a helper program and reads its output
([ADR-0090](https://github.com/rust-works/omni-dev/blob/main/docs/adrs/adr-0090.md),
[secret-commands.md](https://github.com/rust-works/omni-dev/blob/main/docs/secret-commands.md));
both ADRs stay in omni-dev, see the [ADR inventory](adrs/README.md). Only the resolver and
the settings writers may spell either companion. Document
`<NAME>_FILE` and `<NAME>_COMMAND` next to the variable in its operator guide. A secret-shaped name that genuinely must not accept `_FILE`
goes in `EXEMPT_SECRET_ENV_VARS` with its reason. Test the call site's `_FILE`
path with `MapEnv` and `test_support::env::secret_file` (STYLE-0028).

A secret stored as a settings.json field rather than an environment variable
(a named Drive or Gmail account's `client_secret`/`refresh_token`) gets a
`<field>_file` companion. Resolve the pair with
`secret_env::resolve_secret_pair`, labelled with the full settings key, and
never `.clone()` the plain field into a credential.

### Motivation

A secret in an environment variable leaks into `env` listings,
`/proc/<pid>/environ`, shell history and child processes; `_FILE` is the
Docker/Kubernetes way out. Implemented per call site it would drift into
subtly different readers. The grep guards in `secret_env.rs` fail the
build when a new secret-shaped literal is unregistered, when a registered one is
read through a plain accessor, or when a `<NAME>_FILE` would collide with an
existing variable.

---

## STYLE-0031: ADR references in user-visible text

**Tags:** `documentation`, `naming`

### Situation

Naming an Architecture Decision Record in CLI help, an error, or a prompt. This includes
`///` comments that clap turns into help through `#[arg]` or `#[command]` attributes.

### Guidance

Use the bare, zero-padded identifier `ADR-NNNN` consistently, for example `see ADR-0066`
or `see ADR-0069`. Do not print repository paths or Markdown links in terminal text.
Keep Markdown links in ordinary rustdoc comments and documentation, where they resolve.

### Motivation

An installed binary has no repository `docs/` directory, and terminals do not render
Markdown links. A consistent identifier keeps terminal text concise and matches the ADR
titles and inventory used to look up the decision in the project's documentation.

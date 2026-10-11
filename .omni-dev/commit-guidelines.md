# gwi Commit Guidelines

This project follows conventional commit format with specific requirements.

## Severity Levels

| Severity | Sections                                                                |
|----------|-------------------------------------------------------------------------|
| error    | Commit Format, Types, Scopes, Subject Line, Accuracy, Breaking Changes |
| warning  | Body Guidelines                                                         |
| info     | Subject Line Style                                                      |

## Commit Format

```
<type>(<scope>): <description>

[optional body]

[optional footer(s)]
```

Multiple scopes are allowed when a commit spans more than one area.
Separate scopes with a comma. An optional single space after the comma
is permitted; both forms are accepted:

```
<type>(<scope1>,<scope2>): <description>
<type>(<scope1>, <scope2>): <description>
```

Two or more spaces after a comma, or any whitespace before a comma, is
not permitted.

## Types

Required. Must be one of:

| Type       | Use for                                               |
|------------|-------------------------------------------------------|
| `feat`     | New features or enhancements to existing features     |
| `fix`      | Bug fixes                                             |
| `docs`     | Documentation changes only                            |
| `refactor` | Code refactoring without behavior changes             |
| `chore`    | Maintenance tasks, dependency updates, config changes |
| `test`     | Test additions or modifications                       |
| `ci`       | CI/CD pipeline changes                                |
| `build`    | Build system or external dependency changes           |
| `perf`     | Performance improvements                              |
| `style`    | Code style changes (formatting, whitespace)           |

## Scopes

Required. The project-specific scopes are defined in `.omni-dev/scopes.yaml`.
The checker supplies the resolved valid scope set automatically; see the
`VALID SCOPES FOR THIS PROJECT` section of the check prompt for the full list.

In addition to the YAML-defined scopes, this project is a Rust crate, so the
ecosystem default scopes `cargo`, `core`, `lib` and `test` are also
accepted.

Shared utilities in `src/utils.rs` and `src/utils/**` have a dedicated `utils`
scope. Shared Rust infrastructure with no subsystem owner, such as `src/lib.rs`
and `src/test_support.rs`, uses the ecosystem `lib` scope instead. These unowned
infrastructure files are listed in `allow:` in `scopes.yaml` for
`omni-dev config scopes lint`; the utility paths are covered by the `utils`
scope's `file_patterns`.

For multi-scope commits, the scopes are correct when each listed scope
matches at least one modified file. Do not flag scopes as incorrect
when the commit legitimately spans multiple areas.

A one-line `pub mod X;` registration added to `src/lib.rs` counts as
part of scope `X` (or whichever scope owns the new module), not as a
separate scope. Do not require a dedicated scope for `src/lib.rs`
when the only change there is wiring in a new module that already has
its own scope.

## Subject Line

- Use imperative mood: "add feature" not "added feature" or "adds feature"
- Be specific: avoid vague terms like "update", "fix stuff", "changes"

## Subject Line Style

- Use lowercase for the description
- No period at the end

## Accuracy

The commit message must accurately reflect the actual code changes:

- **Type must match changes**: Don't use `feat` for a bug fix, or `fix` for new functionality
- **Scope must match files**: The scope should reflect which area of code was modified
- **Description must be truthful**: Don't claim changes that weren't made
- **Mention significant changes**: If you add error handling, logging, or change behavior, mention it

Only flag accuracy errors when the commit message is clearly and
materially wrong. Do not flag minor terminology differences,
language-specific semantic debates, or cases where the description
is substantially correct even if slightly imprecise. Before reporting
an issue, verify your reasoning is internally consistent — if your
own explanation concludes the commit is actually correct, do not
report it.

## Body Guidelines

For significant changes (>50 lines or architectural changes), include a body:

- Explain what was changed and why
- Describe the approach taken
- Note any breaking changes or migration requirements
- Use bullet points for multiple related changes
- Reference issues in footer: `Closes #123` or `Fixes #456`

## Breaking Changes

For breaking changes:
- Add `!` after type/scope: `feat(cli)!: change output format`
- Include `BREAKING CHANGE:` footer with migration instructions

## Examples

### Simple change
```
fix(gmail): handle empty search results
```

### Feature with body
```
feat(drive): add a command to restore files from trash

Let users restore individual files from Drive Trash without opening
the browser. Apply the same permission checks as the trash command.

- Add the untrash CLI command
- Require write authorization and trash permission
- Refuse folders before making the restore request

Closes #85
```

### Documentation
```
docs(docs): explain drive write leases
```

```
docs(docs): document gmail search output
```

### Multiple scopes
```
feat(cli,gmail): add enriched gmail search output
```

```
fix(drive, utils): apply shared timeouts to drive requests

Use the shared HTTP timeout settings for Drive clients so a stalled
connection or response cannot wait indefinitely.

- Configure connect and per-read timeouts in the shared HTTP helper
- Apply that helper when building Drive clients
```

### Breaking change
```
feat(cli)!: change gmail search json output format

BREAKING CHANGE: The gmail search command's JSON output now wraps
results in a messages object instead of a top-level array. Update
scripts to read the messages field before iterating over results.
```

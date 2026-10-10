# Help and schema text presentation

The #180 audit covers all 169 generated CLI command/argument help screens in short and long
form and all 23 tools and 90 top-level parameter occurrences advertised by MCP tools/list.
It builds on the URL identifier split in #121 and the prose audit in #142.

| Surface | Convention | Reason |
| --- | --- | --- |
| Rustdoc | Markdown code spans and links | Rendered documentation benefits from formatting and resolved links. |
| CLI short/long help | Plain text, explicit clap help/long_help or about/long_about | Terminal output prints Markdown delimiters literally. Short summaries retain the first paragraph; long help retains all paragraphs. |
| MCP parameter descriptions | Plain text, explicit schemars descriptions where rustdoc contains Markdown | Clients may display plain text; values and examples should be directly readable and copyable. |
| Explicit MCP tool descriptions | Retain intentional inline code Markdown | Authored independently of rustdoc; code spans distinguish CLI equivalents, tool names and call examples in clients that render Markdown. |

The CLI audit includes root/profile and account selection, Gmail auth/search/read/thread,
labels, drafts, archives/render/insert, Drive auth/search/read/write/permissions/leases,
and every Docs, Sheets and Slides command. It covers command summaries as well as options
and arguments, including repeated help from reused argument structs. Existing plain URL
identifier overrides remain intact. No generated help retains backticks or unresolved
Markdown links.

The MCP audit covers Gmail account/auth/search/message/thread/label/draft tools, Drive
account/auth/search/dedupe/read, Docs and Sheets info/read, Docs replace/append, Sheets
write/append/clear, and lease acquisition. Shared account descriptions get separate
formatted rustdoc and plain schema text. The 23 explicit tool descriptions retain their
intentional code spans; all 90 top-level parameter occurrences use plain text.

Command names, flags, settings keys, paths, enum values, booleans, JSON and search examples
retain their exact spelling without presentation delimiters. For example, drive edit keeps
the stdin marker -, MIME default application/octet-stream and files.update reference;
Gmail search keeps label:finance after:2026/01/01, the false enrichment default and quota
cost; Sheets values keeps the JSON row array; Docs tab selection keeps drive_docs_info and
tabs[].tab_id. Literal syntax describing Markdown output (such as the quoted-history
omission marker) is preserved as content. Identifier hints, required/optional semantics,
defaults and omission behavior remain unchanged.

Generated-output tests traverse both CLI help formats and inspect MCP tools/list, rather
than only scanning comments. The complete help snapshot records the presentation changes.
Keep explicit text synchronized with its rustdoc source as required by
[STYLE-0008](STYLE_GUIDE.md#style-0008-doc-comments). This presentation pass does not repeat
[the MCP prose-quality audit](mcp-description-audit.md), change nested schemas or introduce
a global Markdown renderer.

The explicit short descriptions for single-paragraph docs omit redundant
`long_help = None` / `long_about = None` resets. Each reset adds another builder
temporary to clap's generated code and can overflow the Windows process's
1 MiB startup stack in debug builds. Windows generated frames still need more
room, so build.rs reserves 8 MiB for the CLI executable (pages commit on demand).
A command-tree regression exercises the Windows reserve and a tighter 1 MiB
budget elsewhere; binary smoke tests verify actual startup, and the help snapshot
verifies identical output.

## Paired-description consistency guard

`cargo test --test description_consistency_test` runs in the default test suite and
parses the CLI and MCP Rust sources with `syn`. It compares explicit clap
`help`/`long_help`, `about`/`long_about` and value help, and schemars parameter
`description` attributes with the rustdoc attached to that same struct, enum,
variant or field. Functions and independently authored MCP `tool` descriptions
are excluded. A failure names the source path, item/parameter, attribute and line,
and shows both texts. The existing generated-help snapshot and MCP tools/list
tests still check the actual outputs.

The test applies only these presentation conventions to both sides:

- Collapse whitespace, including wrapped source lines. Short clap text uses the
  first rustdoc paragraph and may omit its single final period; long clap text and
  schemars descriptions use every paragraph.
- Remove inline code delimiters while preserving their literal contents. Remove
  single/double asterisk emphasis outside code spans, including emphasis around a
  code span. Retained emphasis in older explicit descriptions is also accepted.
- Compare inline Markdown links by readable label. Rustdoc shorthand code links
  such as ``[`LeaseFlags`]`` and the older explicit `[LeaseFlags]` representation
  compare as `LeaseFlags`; only bracketed labels actually linked in that paired
  rustdoc are accepted.
  Literal links and emphasis inside code examples are preserved.

Four source-specific mappings preserve literal output examples rather than
interpreting them as prose formatting: `OutputFormat::Yamls` and
`ReadOutputFormat::Yamls` map the rustdoc code span for `---` to help's quoted `'---'`; `MessageOutputArgs::fold_quotes` and
`RenderCommand::fold_quotes` map code spans for `>` and
`*(N quoted lines omitted)*` to the same help examples in single quotes. These
mappings require the exact example text and exact source item. Changing an example,
a default, or an omission rule on just one side still fails. There are no general
wording exceptions; intentional short/full differences follow paragraph boundaries.

String literals (including raw strings and Rust line continuations) and the two
shared `account_param_doc!()`/`account_param_plain!()` macros are supported. The
guard parses those macro bodies and their explicit Drive imports, so changing a
shared string is checked at each parameter that uses it. Unsupported description
expressions or macro shapes fail rather than silently disappearing from coverage.
Add an explicit syntax resolver and regression when introducing another authoring
form. Explicit `None` disables a derived clap description and has no prose pair;
items without explicit overrides or without rustdoc have no paired text to compare.
This guard is test-only and does not rewrite generated text or render general
Markdown at runtime.

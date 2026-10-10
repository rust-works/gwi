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

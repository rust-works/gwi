# JSON passthrough compatibility

Settings updates and settings import preserve arbitrary unknown JSON through an explicit
read/merge/write boundary ([#273](https://github.com/rust-works/gwi/issues/273)).
Sheets JSON cell input validates original JSON kinds before scalar conversion
([#274](https://github.com/rust-works/gwi/issues/274)). The
[#209](https://github.com/rust-works/gwi/issues/209) audit documented the private
number-key collision. [#275](https://github.com/rust-works/gwi/issues/275) now
preserves selected API payloads and Sheets metadata at explicit raw JSON boundaries.
The ledger's separate preservation guarantee is documented in
[lease ledger JSON](lease-ledger-json.md) and introduced by
[#208](https://github.com/rust-works/gwi/issues/208).

## Confirmed collision

With serde_json's crate-wide `arbitrary_precision` feature, an object whose **first
key** is `$serde_json::private::Number` collides with its internal number protocol:

```json
{"$serde_json::private::Number":"123"}
{"$serde_json::private::Number":"not-a-number"}
{"$serde_json::private::Number":"123","other":"keep"}
```

The first becomes the number `123`, losing its object shape, key and string content
on a rewrite or forwarding operation. The second fails with `invalid number`; the
third fails with `trailing comma` when streamed into Value, or `invalid length`
through buffered flattened fields. The input is valid JSON despite the diagnostic.
These outcomes also occur inside nested objects
and arrays. The ordinary-key-first control
`{"other":"keep","$serde_json::private::Number":"123"}` retains both keys and string
values. Writers using the affected Value boundaries should avoid the private key rather than rely on member order.
A string containing the key's text is safe.

The raw boundaries enable serde_json's crate-wide `raw_value` feature. In
unprotected Value deserializers, a first-key `$serde_json::private::RawValue`
object also collides with serde_json's internal protocol: its string content is
interpreted as JSON rather than retained as a string-valued object. Protected
payload slots explicitly construct both private-key objects. Avoid either
private key in the remaining unprotected consumers.

## Production inventory

The inventory follows deserialization into `Value`, including generic HTTP parsing,
custom deserializers and flattened unknown fields, through subsequent writes/output.
Typed JSON is distinguished from arbitrary metadata; a `Value` in a request builder
or a test fixture is not itself a passthrough boundary.

| Entry points / source files | Preservation contract and observed scope |
| --- | --- |
| Settings env/profile upsert/removal, Gmail/Drive account upsert/removal and default-account updates (`src/utils/settings.rs`) | Read the entire file as a settings JSON tree with explicitly parsed ordered objects and arrays. All ten writer paths preserve the literal objects above, including nested unknown profile metadata. Only scalar leaves use `Value`; typed `Settings::load_from_path` continues to ignore unknown fields. |
| Settings `gwi import` (`src/cli/import.rs`) | The same settings JSON tree preserves unknown fields in selected Gmail/Drive items and the lease settings block, and retains unrelated destination metadata. Valid private-number-key objects in unselected source fields no longer block import. Only selected fields are imported; unrelated source fields are not copied. |
| Sheets filter-view re-creation (`src/drive/sheets/filter.rs`, `FilterCriteria::extra`) | Clearing a sort column re-creates the view with retained unknown criteria. Literal read/build/serialize regressions preserve all four object variants and their nested forms using explicit typed map deserialization. An ordinary masked criteria update leaves untouched columns on the server instead of re-sending them. |
| Sheets chart updates (`src/drive/sheets/embedded_object.rs`, chart `extra` maps in `types.rs`) | Updates send a replacement spec and retain unmodelled fields in `ChartSpec`, `BasicChartSpec`, `BasicChartAxis`, `BasicChartDomain`, `BasicChartSeries`, `PieChartSpec` and `ChartData`. Title-update regressions preserve the literal private-key objects throughout the retained basic-chart metadata; pie-chart metadata has separate round-trip coverage. |
| Sheets slicer snapshots (`SlicerSpec::extra`) | Unknown fields survive read/output serialization, including private-number-key objects, through an explicit raw boundary confirmed by literal HTTP fixtures. Slicer updates use field masks and constructed specs, so they do not wholesale re-send unknown fields. |
| Gmail message MIME `payload` (`src/gmail/types.rs`), Docs unmodelled structural/paragraph arms (`src/drive/docs/types.rs`), Slides `pageSize`/`image` (`src/drive/slides/types.rs`), Sheets cell values (`ValueRange::values`) | These `Value` slots preserve selected API payloads for output or serve as presence discriminators. Field adapters capture raw JSON and explicitly construct objects before Value can interpret its private number protocol. Literal HTTP response parsing/output tests preserve every selected slot, including all unmodelled Docs paragraph arms. They do not promise preservation of every unknown field of the enclosing typed models. Slides' bounded read uses `from_slice` rather than the shared HTTP parser; both use the same wire models and protected field adapters. |
| Docs table suggestion inspection (`Table::deserialize` in `src/drive/docs/types.rs`) | Raw capture precedes a shape-preserving suggestion scan, including unknown nested fields. Typed table fields decode from the original JSON, protecting selected payloads inside cells and nested tables from both private protocols. Unknown metadata is inspected then discarded; only the suggestion flag is retained, without duplicate document prose. |
| Sheets JSON cell input (`src/cli/drive/sheets/values.rs`) | Intended scalar conversion, not arbitrary JSON passthrough. Borrowed JSON slices validate original kinds before scalar conversion. All object/array cells, including private-number objects and nested containers, fail cell validation with row/column diagnostics. Supported numeric precision is retained. No Sheets request is built when this parser fails. |
| Google error parsing (`src/drive/api_client.rs`, `src/gmail/client.rs`) | Temporary `Value` extracts error messages/reasons or quota hints, not JSON forwarding. Parse failures use existing fallback diagnostics; no unknown-object retention contract. |
| Drive sync manifest, Gmail archive state/manifest/insert ledger, OAuth client-secret import, Chrome Local State, request/audit log records | Typed models (or raw body strings), not arbitrary unknown-JSON retention. Unknown typed fields are ignored or rejected according to their model; they are not merged back as Value. |
| MCP tools and output truncation | Tools consume typed parameters and reuse the service/settings/cell parsers above. Truncation operates on rendered text; test-only parsed markers and constructed JSON do not add an arbitrary JSON input boundary. |

## Settings safety and supported guarantees

Settings updates retain unknown objects, keys and strings, including the private-number
objects above. The boundary validates JSON with serde_json's streaming `IgnoredAny` scanner and uses
its byte offsets to parse object members and array elements as source slices before
building an ordered tree, avoiding Value's object/number protocol. The settings parser does not depend on `raw_value`; the ledger and selected API payload boundaries
enable that crate-wide feature, with the unprotected-consumer limitation described above.
Scalar numbers retain the crate's `arbitrary_precision` behavior; large integers and
high-precision decimals remain exact. Deletions retain the relative order of surviving
object members. Known boolean and numeric settings retain their typed behavior.

Settings import leaves its source untouched and dry runs write nothing. Import equality
compares JSON structurally: whitespace, string escape spelling and object member order
do not create conflicts, while array order and differing scalar values do. Repeated
imports are idempotent. Differing items remain conflicts unless `--force` replaces them.
Malformed JSON is rejected before writing, including forced import; destination bytes
remain unchanged. Nesting is bounded to prevent excessive recursive traversal. The
existing hardened writer retains its directory/file permission and write handling.
No byte/whitespace/duplicate-key guarantee is made. Public account-update helpers still
accept constructed `Value` arguments; they cannot recover data already lost by a caller
parsing arbitrary metadata into `Value` before calling them.

These guarantees concern settings; `gwi import` handles settings and the lease ledger
independently, so a settings error does not prevent an attempted ledger import.

Sheets' outer `extra` maps are `BTreeMap`s and sort their unknown keys; nested Value
objects retain insertion order. This is not the ledger's outer-key ordering contract.

## Selected payload guarantees

Protected slots retain object shape, keys and string contents, large integers,
high-precision decimals and nested object member order. Known fields still use
their typed deserializers, with the same defaults, required fields and duplicate
known-field rejection. Missing or null optional payloads remain absent. Sheets
cell matrices remain ragged and reject malformed matrix shapes. Unknown payload
nesting is bounded to 128 levels to avoid unbounded recursion. No guarantee is
made for whitespace or duplicate unknown keys, or for ignored unknown fields of
an enclosing typed model. Docs tables capture raw JSON before suggestion scanning,
so these guarantees also apply to selected payloads inside cells and nested tables.
The temporary inspection tree uses the same 128-level bound and detects pending
suggestions in unmodelled metadata, including around literal private-key objects.
Typed table fields decode from the original JSON and retain validation/defaults;
duplicate known table fields are rejected instead of collapsed by an intermediate.
Unknown metadata is discarded after inspection.
Chart/filter serialization and request field masks are
unchanged; slicer updates still construct only masked fields.

## Deferred implementation

No current writer is known to produce this unusual key. Retaining `arbitrary_precision`
avoids regressing [#95](https://github.com/rust-works/gwi/issues/95). Docs table
suggestion scanning is protected by
[#286](https://github.com/rust-works/gwi/issues/286). The Sheets scalar validator
already checks raw JSON kinds before Value conversion (#274), and the selected payload
boundaries are implemented by #275. Simply adding raw values to flattened fields does
not avoid serde's buffering.

The audit tests use literal JSON inputs and independently constructed expectations
in [`src/test_support.rs`](../src/test_support.rs); the Sheets cell rejection
regressions keep their own literal inputs in the parser tests. This avoids a pre-parsed fixture that
would hide the collision. They exercise actual settings writers/import, Sheets merge
and validation functions, and local HTTP response parsing/serialization. No live
Google services or user credentials are used.

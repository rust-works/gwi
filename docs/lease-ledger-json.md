# Lease ledger JSON compatibility

The lease ledger preserves unknown row fields and unknown fields inside both backup
variants on import and every rewrite. Unknown keys retain their relative order, including
inside nested objects; large integers and high-precision decimals retain their precision.
Known fields serialize in declaration order before unknown fields. Whitespace, duplicate
object keys and the original placement of unknown fields among known fields are not
preserved.

## Literal private number keys

The explicit ledger deserialization boundary introduced for
[#208](https://github.com/rust-works/gwi/issues/208) preserves these valid JSON objects,
including their keys and string contents:

```json
{"$serde_json::private::Number":"123"}
{"$serde_json::private::Number":"not-a-number"}
{"$serde_json::private::Number":"123","other":"keep"}
```

They survive ledger load/save, locked mutations and `gwi import` at row level, inside
both `bytes` and `drive_copy` backup metadata, and recursively inside arrays and objects.
The key is not reserved by gwi, regardless of member order.

Previously, [#143](https://github.com/rust-works/gwi/issues/143) accepted and documented
serde_json's private number protocol collision: the first object became the number `123`,
and the other two prevented the ledger from loading. The new boundary supersedes that
limitation for ledger files. Already corrupted rewrites cannot reconstruct the original
object from a number; recovering that metadata still requires an original copy or backup.

## Boundary and import equality

The loader first reads ordered raw JSON object entries, separates known row and backup
fields, and deserializes those fields into the existing typed records. It recursively
constructs unknown objects and arrays explicitly, parsing only primitive values through
serde_json's Value deserializer, then attaches the unknown values to the records. This
avoids the private number protocol and serde's flatten/internal-tag buffering. Simply
putting raw values into flattened fields would not provide that boundary.

Unknown values retain the existing parsed-value equality used by import. Insignificant
whitespace, equivalent string escapes and object member order do not cause conflicts.
Array order, types and contents remain significant; numbers keep serde_json's existing
arbitrary-precision equality semantics. Different metadata for the same token still causes
a conflict unless `--force` applies. Release propagation and the prohibition on reviving
released leases remain unchanged. Import never rewrites its source file.

Invalid known fields, unknown backup kinds, malformed JSON and excessively nested unknown
metadata still reject the whole ledger. The error identifies the line and asks the user
to fix it or restore the ledger from a backup. A failed load leaves the file untouched;
`--force` on import does not bypass parsing. Unknown metadata cannot change the meaning
of known backup fields or add support for new backup kinds.

## Scope

Both `preserve_order` and `arbitrary_precision` remain enabled, retaining
[#95](https://github.com/rust-works/gwi/issues/95)'s precision and order guarantees.
The additional `raw_value` feature enables the explicit ledger boundary without changing
other JSON consumers. Paths that deserialize directly into `serde_json::Value` can still
encounter the private number collision; this ledger change does not promise arbitrary
object passthrough for those paths. The [non-ledger JSON audit](json-passthrough.md)
characterizes those consumers and records their accepted scope.

Preservation regressions in
[`src/drive/lease/ledger.rs`](../src/drive/lease/ledger.rs) cover all three literal values
at row level and inside both backup variants, nested arrays/objects, save and locked
mutation, ordinary first keys, large-number precision/order and known numeric fields.
Import unit and CLI tests cover copying, idempotence, formatting-independent equality,
conflicts/forced replacement, source immutability and release authority rules.

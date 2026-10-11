# Lease ledger JSON compatibility

The lease ledger preserves unknown row fields and unknown fields inside both backup
variants on import and every rewrite. Unknown keys retain their relative order, including
inside nested objects; large integers and high-precision decimals retain their precision.
Known fields serialize in declaration order before unknown fields. Whitespace and the
original placement of unknown fields among known fields are not preserved.

## Accepted private number key limitation

**Decision for [#143](https://github.com/rust-works/gwi/issues/143): retain the existing
serde_json representation and explicitly accept this limitation.** Arbitrary valid JSON
in unknown metadata is not guaranteed to round-trip.

With serde_json's `arbitrary_precision` feature, an object whose **first key** is
`$serde_json::private::Number` collides with the library's internal number representation.
This key is not deliberately reserved by gwi. These three valid JSON values illustrate
the collision when used as an unknown field such as `future`:

```json
{"$serde_json::private::Number":"123"}
{"$serde_json::private::Number":"not-a-number"}
{"$serde_json::private::Number":"123","other":"keep"}
```

- The first object loads as the number `123`. A save or lease mutation writes that number,
  losing the object shape, its key and the original string value. Import also carries the
  number rather than the original object.
- The second object fails to load with `invalid number`.
- The third object fails to load with `invalid length 2, expected 1 element in map`.

A parse failure rejects the whole ledger, leaving the file untouched. It blocks operations
that depend on loading that ledger, including import and leased writes. The load error
identifies the line and asks the user to fix it or restore the ledger from a backup.
`--force` on import does not bypass JSON parsing.

The limitation applies to unknown row fields and unknown metadata in both `bytes` and
`drive_copy` backups. It also applies recursively to objects inside arrays or other
objects. An ordinary object key before the private number key avoids this particular
collision, but writers should avoid this key entirely rather than rely on member order.
Existing corrupted rewrites cannot reconstruct the original object from the resulting
number; recovering that metadata requires an original copy or backup.

## Scope and rationale

Both `preserve_order` and `arbitrary_precision` remain enabled. Disabling arbitrary
precision would restore this object shape at the cost of rounding large integers and
precise decimals, regressing [#95](https://github.com/rust-works/gwi/issues/95).
No current ledger writer is known to use the unusual key, so a custom parser is deferred.

A future fix must preserve unknown JSON through an explicit deserialization boundary,
for example using raw JSON, and keep import equality/conflict semantics. Adding raw values
to a flattened field alone is insufficient because serde buffers flattened and internally
tagged data. Such a change must cover the row and both backup variants and retain the
precision/order guarantees above.

The feature is crate-wide: other paths that deserialize JSON objects into
`serde_json::Value` can encounter the same collision, not only the lease ledger or
flattened fields. For example, the
[settings reader](../src/utils/settings.rs) also loads a generic JSON value to preserve
unknown settings on rewrites. The [non-ledger JSON audit](json-passthrough.md)
characterizes those consumers and records their accepted scope. This decision makes
no feature changes and does not promise arbitrary object passthrough for those consumers. Typed strings and ordinary known ledger fields
are not interpreted as private number objects merely because they contain this text.

The characterization tests in
[`src/drive/lease/ledger.rs`](../src/drive/lease/ledger.rs) exercise all three values at
row level and inside both backup variants, including nested arrays, save/mutation behavior,
parse-error file preservation, and an ordinary first key. The existing precision/order
regression continues to cover supported unknown values.

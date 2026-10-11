# Upstream report draft (#295)

Ready-to-file text for [rust-lang/rust](https://github.com/rust-lang/rust/issues/new/choose),
labelled `A-code-coverage`. **Not yet filed**: filing publishes under a person's GitHub
account, so it waits for that person's decision. When it is filed, record the link in
[the investigation](../sheets-mcp-coverage-delta.md#upstream-disposition-295) and in
[#295](https://github.com/rust-works/gwi/issues/295). The text below is deliberately
self-contained; it does not assume the reader knows gwi or rmcp.

Searches for an equivalent report found none (see the investigation). The closest are
[#123567](https://github.com/rust-lang/rust/issues/123567),
[#131119](https://github.com/rust-lang/rust/issues/131119) and
[#147434](https://github.com/rust-lang/rust/issues/147434), which concern information lost or
shifted in proc-macro output, not a region gained from punctuation elsewhere in the attribute.

---

**Title:** Coverage: editing a string in an attribute macro adds a zero-hit region for generated `panic!` code, because `span_through_char(span, '!')` scans the whole attribute

## Observation

A procedural attribute macro generates a function containing an `unwrap_or_else(|e| { panic!(..) })`
closure. The closure's tokens carry the attribute's call-site span (default `quote!` span). The
attribute argument is a string literal. Changing only that literal from `"x"` to `"x!"`:

- changes no control flow: MIR before `InstrumentCoverage` is identical apart from span
  coordinates, and the closure is defined in the LLVM IR of both variants;
- changes no runtime behaviour: the test passes in both and the closure never runs;
- changes the coverage mapping: `"x"` yields no region for the closure, `"x!"` yields one
  zero-hit region `[8, 1, 8, 27, 0, 0, 0, 0]` (line 8, column 1 to the character just after the
  `!`), so `llvm-cov` reports an uncovered line that was not reported before.

In a real crate (`rmcp` 3.5.1's `#[tool(description = "...")]`) this appeared as eleven extra
uncovered lines when a multi-line description gained `Sheet1!A1:C10`.

## Reproducer

Two crates, no dependencies beyond `syn`/`quote`/`proc-macro2`.

`macros/src/lib.rs` (`proc-macro = true`):

```rust
use proc_macro::TokenStream;
use quote::{format_ident, quote};

#[proc_macro_attribute]
pub fn metadata(attr: TokenStream, item: TokenStream) -> TokenStream {
    let description = proc_macro2::TokenStream::from(attr);
    let function = syn::parse_macro_input!(item as syn::ItemFn);
    let ident = format_ident!("{}_metadata", function.sig.ident);
    quote! {
        #function
        pub fn #ident() -> (&'static str, bool) {
            (#description, schema().unwrap_or_else(|e| {
                panic!("Invalid schema: {}", e)
            }))
        }
    }
    .into()
}
```

`case/src/lib.rs`:

```rust
fn schema() -> Result<bool, &'static str> {
    Ok(true)
}

#[span_macro::metadata("x")] // vs. #[span_macro::metadata("x!")]
pub fn handler() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_accepts_the_schema() {
        assert!(handler_metadata().1);
    }
}
```

With `cargo-llvm-cov` 0.9.1 (`cargo llvm-cov --json`), the function whose name contains
`handler_metadata` is absent for `"x"` and present with count 0 for `"x!"`.

The complete fixture, a replay driver that builds both variants in separate directories and
retains source, logs, lcov, LLVM JSON, expansion, LLVM IR and MIR before/after
`InstrumentCoverage`, and raw results are at
<https://github.com/rust-works/gwi/tree/36d876b3ec0c84d70602faddfc49b8c3ccd5172d/docs/repro-schema-coverage>.

## Versions

Same result in every row (plain: no region; bang: one zero-hit region `[8,1,8,27,0,0,0,0]`):

| Toolchain | Commit | LLVM | Host |
| --- | --- | --- | --- |
| 1.99.0 stable | `b940084d7eb6a299eb4bfeb8e34901bc051e7ac4` | 23.1.1 | x86_64-unknown-linux-gnu, aarch64-apple-darwin |
| 1.100.0-beta.4 | `a2eb6097d22d908142aa4a067bc2cb70860a95b7` | 23.1.1 | aarch64-apple-darwin |
| 1.101.0-nightly (2026-10-10) | `6f5da406b56c2db6607202b63daee73aa8c61297` | 23.1.3 | aarch64-apple-darwin |

Beta and nightly both include #161517 (hybrid HIR-aware coverage spans).

## Suspected cause (a diagnosis, not an observation)

In `single_covspan_for_child_context`
(`compiler/rustc_mir_transform/src/coverage/spans.rs`), a bang-macro expansion's call-site span is
shortened with `source_map().span_through_char(span, '!')`, intending to keep a macro name and its
`!`. Here the `panic!` tokens' call-site span covers the **whole attribute**, so the search finds a
`!` inside the string literal. Without a `!`, the span is unchanged and equals the closure body,
and `extract_refined_covspans` discards spans that fill the whole body, leaving no mapping. With a
`!`, the span is shortened, survives, and becomes a region ending just after that `!`. The
predicted region matches the observed end column in both the reduced case and the real crate.

I have not confirmed this by patching the compiler; it is consistent with the source and the
retained MIR (a `coverage Code { bcb0 }` mapping and a `VirtualCounter` appear only in the bang
variant).

## Questions

- Is a macro call-site span that is not itself `name!(...)` (an attribute, or any span not
  beginning at the macro name) expected to reach this branch?
- If so, should the search for `!` be limited to the start of the span (for example, only if the
  span's text begins with the macro path), rather than anywhere in it?
- Otherwise, is this within the known limitations of proc-macro coverage (see #147434, where the maintainers describe proc-macro spans as fragile)?

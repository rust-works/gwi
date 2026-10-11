//! Reproduces call-site attribution for a generated schema-error closure.

use proc_macro::TokenStream;
use quote::{format_ident, quote};

/// Generates metadata while preserving the handler identifier's span.
#[proc_macro_attribute]
pub fn metadata(attr: TokenStream, item: TokenStream) -> TokenStream {
    let description = proc_macro2::TokenStream::from(attr);
    let function = syn::parse_macro_input!(item as syn::ItemFn);
    // Like rmcp, preserve the handler identifier's span for the metadata name.
    let ident = format_ident!("{}_metadata", function.sig.ident);
    // The generated closure and panic use quote's default call-site span.
    quote! {
        #function
        /// Returns the description and a validated schema.
        pub fn #ident() -> (&'static str, bool) {
            (#description, schema().unwrap_or_else(|e| {
                panic!("Invalid schema: {}", e)
            }))
        }
    }
    .into()
}

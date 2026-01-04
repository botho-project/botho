// Copyright (c) 2018-2022 The Botho Foundation

//! Procedural macros for test and benchmark functions that require a Logger.
//!
//! With tracing, the Logger is a compatibility type and doesn't require
//! special scope management. These macros create a Logger and pass it
//! to the wrapped function for API compatibility.

#![feature(proc_macro_diagnostic)]

extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

/// Attribute macro for tests that require a Logger parameter.
///
/// Usage:
/// ```ignore
/// #[test_with_logger]
/// fn my_test(logger: Logger) {
///     // test code using logger
/// }
/// ```
#[proc_macro_attribute]
pub fn test_with_logger(_attr: TokenStream, item: TokenStream) -> TokenStream {
    impl_with_logger(item, quote!(), quote!())
}

/// Attribute macro for benchmarks that require a Logger parameter.
#[proc_macro_attribute]
pub fn bench_with_logger(_attr: TokenStream, item: TokenStream) -> TokenStream {
    impl_with_logger(item, quote!(b: &mut Bencher), quote!(,b))
}

fn impl_with_logger(item: TokenStream, params: TokenStream2, args: TokenStream2) -> TokenStream {
    let mut original_fn = syn::parse_macro_input!(item as syn::ItemFn);

    let orig_ident = original_fn.sig.ident.clone();
    let orig_name = orig_ident.to_string();

    let new_ident = quote::format_ident!("__wrapped_{}", orig_ident);
    original_fn.sig.ident = new_ident.clone();

    // With tracing, we don't need slog_scope - just create a Logger and call the
    // function
    let mut new_fn: syn::ItemFn = syn::parse_quote! {
        #[test]
        fn #orig_ident(#params) {
            let test_name = format!("{}::{}", module_path!(), #orig_name);
            let logger = bth_common::logger::create_test_logger(test_name);
            #new_ident(logger #args);
        }
    };
    // Move other attributes to the new method.
    new_fn.attrs.append(&mut original_fn.attrs);

    quote! {
        #new_fn
        #original_fn
    }
    .into()
}

/// Attribute macro for async tests that require a Logger parameter.
///
/// Usage:
/// ```ignore
/// #[async_test_with_logger]
/// async fn my_async_test(logger: Logger) {
///     // async test code using logger
/// }
/// ```
#[proc_macro_attribute]
pub fn async_test_with_logger(attrs: TokenStream, item: TokenStream) -> TokenStream {
    let mut original_fn = syn::parse_macro_input!(item as syn::ItemFn);

    let orig_ident = original_fn.sig.ident.clone();
    let orig_name = orig_ident.to_string();

    let new_ident = quote::format_ident!("__wrapped_{}", orig_ident);
    original_fn.sig.ident = new_ident.clone();

    let attrs: TokenStream2 = attrs.into();

    // With tracing, we don't need slog_scope - just create a Logger and call the
    // function
    let mut new_fn: syn::ItemFn = syn::parse_quote! {
        #[tokio::test(#attrs)]
        async fn #orig_ident() {
            let test_name = format!("{}::{}", module_path!(), #orig_name);
            let logger = bth_common::logger::create_test_logger(test_name);
            #new_ident(logger).await;
        }
    };
    // Move other attributes to the new method.
    new_fn.attrs.append(&mut original_fn.attrs);

    quote! {
        #new_fn
        #original_fn
    }
    .into()
}

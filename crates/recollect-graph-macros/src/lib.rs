//! `#[cell]`: turns a plain struct into the type of a cell named after it in snake case, or as given by
//! `#[cell("name")]`.
//!
//! `#[component]`: turns a unit struct into a component's name, the same way.

use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, LitStr, parse_macro_input};

fn snake_case(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

fn name_of(attr: TokenStream, input: &DeriveInput) -> Result<String, TokenStream> {
    if attr.is_empty() {
        return Ok(snake_case(&input.ident.to_string()));
    }
    syn::parse::<LitStr>(attr).map(|l| l.value()).map_err(|e| e.to_compile_error().into())
}

#[proc_macro_attribute]
pub fn cell(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    let name = match name_of(attr, &input) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let ident = &input.ident;
    quote! {
        #[derive(::core::clone::Clone, ::core::cmp::PartialEq, ::core::fmt::Debug, ::serde::Serialize, ::serde::Deserialize)]
        #input

        impl ::recollect_graph::CellType for #ident {
            const NAME: &'static str = #name;
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn component(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    let name = match name_of(attr, &input) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let ident = &input.ident;
    quote! {
        #[derive(::core::clone::Clone, ::core::marker::Copy, ::core::fmt::Debug, ::core::default::Default)]
        #input

        impl ::recollect_graph::ComponentType for #ident {
            const NAME: &'static str = #name;
        }
    }
    .into()
}

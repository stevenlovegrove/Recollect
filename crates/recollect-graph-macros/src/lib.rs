//! `#[component]`: turns a plain struct into a component named after it in snake case, or as
//! given by `#[component("name")]`.
//!
//! `#[rule("key", incoming(..), outgoing(..), scans(..))]`: turns a struct, whose fields are its
//! parameters, into a rule that can be written down and looked up again by that key.

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{DeriveInput, Ident, LitStr, Path, Token, parenthesized, parse_macro_input};

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

#[proc_macro_attribute]
pub fn component(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    let ident = &input.ident;
    let name = if attr.is_empty() { snake_case(&ident.to_string()) } else { parse_macro_input!(attr as LitStr).value() };
    quote! {
        #[derive(::core::clone::Clone, ::core::cmp::PartialEq, ::core::fmt::Debug, ::serde::Serialize, ::serde::Deserialize)]
        #input

        impl ::recollect_graph::Component for #ident {
            const NAME: &'static str = #name;
        }
    }
    .into()
}

struct RuleArgs {
    key: LitStr,
    groups: Vec<(Ident, Vec<Path>)>,
}

impl Parse for RuleArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let key: LitStr = input.parse()?;
        let mut groups = Vec::new();
        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let name: Ident = input.parse()?;
            if !["incoming", "outgoing", "scans"].contains(&name.to_string().as_str()) {
                return Err(syn::Error::new(name.span(), "expected incoming, outgoing or scans"));
            }
            let content;
            parenthesized!(content in input);
            let paths = Punctuated::<Path, Token![,]>::parse_terminated(&content)?;
            // Saying `outgoing` twice is the same as listing both.
            match groups.iter_mut().find(|(n, _): &&mut (Ident, Vec<Path>)| *n == name) {
                Some((_, existing)) => existing.extend(paths),
                None => groups.push((name, paths.into_iter().collect())),
            }
        }
        Ok(RuleArgs { key, groups })
    }
}

#[proc_macro_attribute]
pub fn rule(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as RuleArgs);
    let input = parse_macro_input!(item as DeriveInput);
    let ident = &input.ident;
    let key = &args.key;
    let fields = args.groups.iter().map(|(name, paths)| {
        quote! { #name: vec![#(<#paths as ::recollect_graph::Component>::NAME.to_string()),*] }
    });
    quote! {
        #[derive(::core::clone::Clone, ::core::fmt::Debug, ::serde::Serialize, ::serde::Deserialize, ::recollect_graph::schemars::JsonSchema)]
        #[schemars(crate = "::recollect_graph::schemars")]
        #input

        impl ::recollect_graph::RuleMeta for #ident {
            const KEY: &'static str = #key;
            fn declared_inputs() -> ::recollect_graph::Inputs {
                ::recollect_graph::Inputs { #(#fields,)* ..::core::default::Default::default() }
            }
        }

        impl ::core::convert::From<#ident> for ::recollect_graph::Storage<::recollect_graph::OutputOf<#ident>> {
            fn from(r: #ident) -> Self {
                ::recollect_graph::Storage::Rule(::recollect_graph::into_rule(r))
            }
        }
    }
    .into()
}

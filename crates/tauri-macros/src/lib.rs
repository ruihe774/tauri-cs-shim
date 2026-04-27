//! Procedural macros for the `tauri` debug shim.
//!
//! M1 scope:
//! - `#[command]` — generates a JSON-deserializing wrapper for each user fn.
//! - `generate_handler!(a, b, ...)` — dispatch closure mapping command name to wrapper.
//! - `generate_context!()` — stub, expands to `::tauri::Context`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::parse::Parser;
use syn::{FnArg, ItemFn, Pat, parse_macro_input, punctuated::Punctuated};

#[proc_macro_attribute]
pub fn command(_attrs: TokenStream, input: TokenStream) -> TokenStream {
    let func = parse_macro_input!(input as ItemFn);
    expand_command(func)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_command(func: ItemFn) -> syn::Result<TokenStream2> {
    let fn_name = &func.sig.ident;
    let wrapper_ident = format_ident!("__tauri_cmd_{}", fn_name);
    let is_async = func.sig.asyncness.is_some();
    let vis = &func.vis;

    let mut deserialize_stmts = Vec::new();
    let mut call_args = Vec::new();

    for arg in &func.sig.inputs {
        let FnArg::Typed(pt) = arg else {
            return Err(syn::Error::new_spanned(
                arg,
                "self receivers are not supported on #[command] functions",
            ));
        };
        let Pat::Ident(pi) = &*pt.pat else {
            return Err(syn::Error::new_spanned(
                &pt.pat,
                "only simple identifier patterns are supported on #[command] parameters",
            ));
        };
        let arg_ident = &pi.ident;
        let arg_ty = &pt.ty;
        let json_key = snake_to_camel(&arg_ident.to_string());

        deserialize_stmts.push(quote! {
            let #arg_ident: #arg_ty = match req.body().get(#json_key) {
                Some(v) => match ::tauri::__private::serde_json::from_value(v.clone()) {
                    Ok(val) => val,
                    Err(e) => return ::std::result::Result::Err(
                        ::tauri::ipc::InvokeError::from_message(
                            format!("invalid argument `{}`: {}", #json_key, e)
                        )
                    ),
                },
                None => return ::std::result::Result::Err(
                    ::tauri::ipc::InvokeError::from_message(
                        format!("missing argument `{}`", #json_key)
                    )
                ),
            };
        });
        call_args.push(quote!(#arg_ident));
    }

    let invocation = if is_async {
        quote! { #fn_name(#(#call_args),*).await }
    } else {
        quote! { #fn_name(#(#call_args),*) }
    };

    let return_handling = if returns_result(&func.sig.output) {
        quote! {
            let __ret = #invocation;
            match __ret {
                ::std::result::Result::Ok(v) => match ::tauri::__private::serde_json::to_value(&v) {
                    Ok(jv) => ::std::result::Result::Ok(jv),
                    Err(e) => ::std::result::Result::Err(
                        ::tauri::ipc::InvokeError::from_message(
                            format!("failed to serialize return value: {}", e)
                        )
                    ),
                },
                ::std::result::Result::Err(e) => match ::tauri::__private::serde_json::to_value(&e) {
                    Ok(jv) => ::std::result::Result::Err(::tauri::ipc::InvokeError::new(jv)),
                    Err(se) => ::std::result::Result::Err(
                        ::tauri::ipc::InvokeError::from_message(
                            format!("failed to serialize error value: {}", se)
                        )
                    ),
                },
            }
        }
    } else {
        quote! {
            let __ret = #invocation;
            match ::tauri::__private::serde_json::to_value(&__ret) {
                Ok(v) => ::std::result::Result::Ok(v),
                Err(e) => ::std::result::Result::Err(
                    ::tauri::ipc::InvokeError::from_message(
                        format!("failed to serialize return value: {}", e)
                    )
                ),
            }
        }
    };

    let wrapper = quote! {
        #[doc(hidden)]
        #[allow(non_snake_case)]
        #vis fn #wrapper_ident(
            req: ::tauri::ipc::CommandRequest,
        ) -> ::std::pin::Pin<
            ::std::boxed::Box<
                dyn ::std::future::Future<
                    Output = ::std::result::Result<
                        ::tauri::__private::serde_json::Value,
                        ::tauri::ipc::InvokeError,
                    >,
                > + ::std::marker::Send,
            >,
        > {
            ::std::boxed::Box::pin(async move {
                #(#deserialize_stmts)*
                #return_handling
            })
        }
    };

    Ok(quote! {
        #func
        #wrapper
    })
}

#[proc_macro]
pub fn generate_handler(input: TokenStream) -> TokenStream {
    let parser = Punctuated::<syn::Path, syn::Token![,]>::parse_terminated;
    let paths = match parser.parse(input) {
        Ok(p) => p,
        Err(e) => return e.into_compile_error().into(),
    };

    let arms = paths.iter().map(|path| {
        let last = path.segments.last().expect("path must have at least one segment");
        let cmd_name = last.ident.to_string();
        let wrapper_ident = format_ident!("__tauri_cmd_{}", last.ident);
        let mut wrapper_path = path.clone();
        wrapper_path.segments.last_mut().unwrap().ident = wrapper_ident;
        wrapper_path.segments.last_mut().unwrap().arguments = syn::PathArguments::None;
        quote! {
            #cmd_name => #wrapper_path(req),
        }
    });

    quote! {
        {
            fn __tauri_invoke_handler(
                req: ::tauri::ipc::CommandRequest,
            ) -> ::std::pin::Pin<
                ::std::boxed::Box<
                    dyn ::std::future::Future<
                        Output = ::std::result::Result<
                            ::tauri::__private::serde_json::Value,
                            ::tauri::ipc::InvokeError,
                        >,
                    > + ::std::marker::Send,
                >,
            > {
                match req.name() {
                    #(#arms)*
                    _ => {
                        let unknown = req.name().to_string();
                        ::std::boxed::Box::pin(async move {
                            ::std::result::Result::Err(
                                ::tauri::ipc::InvokeError::not_found(unknown)
                            )
                        })
                    }
                }
            }
            __tauri_invoke_handler
        }
    }
    .into()
}

#[proc_macro]
pub fn generate_context(_input: TokenStream) -> TokenStream {
    quote!(::tauri::Context::new()).into()
}

/// Syntactic match for `Result<...>` (and any `::path::Result<...>` re-export).
/// We can't resolve types in a proc-macro, so this matches the trailing path
/// segment — the same fragility upstream Tauri lives with.
fn returns_result(ret: &syn::ReturnType) -> bool {
    match ret {
        syn::ReturnType::Default => false,
        syn::ReturnType::Type(_, ty) => match &**ty {
            syn::Type::Path(p) => p
                .path
                .segments
                .last()
                .is_some_and(|s| s.ident == "Result"),
            _ => false,
        },
    }
}

fn snake_to_camel(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut upper_next = false;
    for ch in snake.chars() {
        if ch == '_' {
            upper_next = true;
        } else if upper_next {
            out.extend(ch.to_uppercase());
            upper_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}

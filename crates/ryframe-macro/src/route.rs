use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Attribute, FnArg, GenericArgument, ItemFn, LitStr, PathArguments, Token, Type, parse::Parser,
    parse_macro_input, punctuated::Punctuated,
};

pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

pub fn expand_route(method: HttpMethod, args: TokenStream, input: TokenStream) -> TokenStream {
    let paths: Vec<LitStr> = match Punctuated::<LitStr, Token![,]>::parse_terminated.parse(args) {
        Ok(paths) if !paths.is_empty() => paths.into_iter().collect(),
        Ok(_) => {
            return syn::Error::new(proc_macro2::Span::call_site(), "route path is required")
                .to_compile_error()
                .into();
        }
        Err(error) => return error.to_compile_error().into(),
    };
    let mut function = parse_macro_input!(input as ItemFn);
    let (permission, _capability) = match take_route_contract_attributes(&mut function.attrs) {
        Ok(contract) => contract,
        Err(error) => return error.to_compile_error().into(),
    };
    let state_type = infer_state_type(&function).unwrap_or_else(|| syn::parse_quote!(()));
    let function_name = &function.sig.ident;
    let visibility = &function.vis;
    let route_function_name = format_ident!("__route_{}", function_name);
    let method_function = match method {
        HttpMethod::Get => quote!(::axum::routing::get),
        HttpMethod::Post => quote!(::axum::routing::post),
        HttpMethod::Put => quote!(::axum::routing::put),
        HttpMethod::Patch => quote!(::axum::routing::patch),
        HttpMethod::Delete => quote!(::axum::routing::delete),
    };
    let method_router = if let Some(permission) = permission {
        quote! {
            crate::__macro_support::perm_route(
                #method_function(#function_name),
                #permission,
            )
        }
    } else {
        quote!(#method_function(#function_name))
    };

    quote! {
        #function

        #visibility fn #route_function_name() -> ::axum::Router<#state_type> {
            let router = ::axum::Router::<#state_type>::new();
            #(let router = router.route(#paths, #method_router);)*
            router
        }
    }
    .into()
}

fn take_route_contract_attributes(
    attributes: &mut Vec<Attribute>,
) -> syn::Result<(Option<LitStr>, Option<LitStr>)> {
    let mut permission = None;
    let mut capability = None;
    let mut retained = Vec::with_capacity(attributes.len());

    for attribute in attributes.drain(..) {
        let marker = attribute
            .path()
            .segments
            .last()
            .map(|segment| segment.ident.to_string());
        if marker.as_deref() != Some("perm") && marker.as_deref() != Some("capability") {
            retained.push(attribute);
            continue;
        }
        let value = attribute.parse_args::<LitStr>()?;
        if value.value().trim() != value.value() || value.value().is_empty() {
            return Err(syn::Error::new_spanned(
                attribute,
                "route marker must be non-empty and trimmed",
            ));
        }
        if marker.as_deref() == Some("perm") {
            if permission.is_some() {
                return Err(syn::Error::new_spanned(
                    attribute,
                    "only one #[perm] attribute is allowed per route handler",
                ));
            }
            permission = Some(value);
        } else {
            if capability.is_some() {
                return Err(syn::Error::new_spanned(
                    attribute,
                    "only one #[capability] attribute is allowed per route handler",
                ));
            }
            capability = Some(value);
        }
    }

    *attributes = retained;
    Ok((permission, capability))
}

fn infer_state_type(function: &ItemFn) -> Option<Type> {
    function.sig.inputs.iter().find_map(|argument| {
        let FnArg::Typed(argument) = argument else {
            return None;
        };
        let Type::Path(path) = argument.ty.as_ref() else {
            return None;
        };
        let segment = path.path.segments.last()?;
        if segment.ident != "State" {
            return None;
        }
        let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
            return None;
        };
        arguments.args.iter().find_map(|argument| match argument {
            GenericArgument::Type(state_type) => Some(state_type.clone()),
            _ => None,
        })
    })
}

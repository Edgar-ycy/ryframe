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
    let permission = match take_permission_attribute(&mut function.attrs) {
        Ok(permission) => permission,
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
        HttpMethod::Delete => quote!(::axum::routing::delete),
    };
    let method_router = if let Some(permission) = permission {
        quote! {
            ::ryframe_auth::middleware::perm_route(
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

fn take_permission_attribute(attributes: &mut Vec<Attribute>) -> syn::Result<Option<LitStr>> {
    let mut permission = None;
    let mut retained = Vec::with_capacity(attributes.len());

    for attribute in attributes.drain(..) {
        let is_permission = attribute
            .path()
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "perm");
        if !is_permission {
            retained.push(attribute);
            continue;
        }
        if permission.is_some() {
            return Err(syn::Error::new_spanned(
                attribute,
                "only one #[perm] attribute is allowed per route handler",
            ));
        }
        permission = Some(attribute.parse_args::<LitStr>()?);
    }

    *attributes = retained;
    Ok(permission)
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

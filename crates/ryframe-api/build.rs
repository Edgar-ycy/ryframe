use std::{
    collections::BTreeSet,
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use syn::{Attribute, Item, LitStr, Token, punctuated::Punctuated};

type RouteCapabilityBinding = (String, String, String, String, String, Option<String>);

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed=RYFRAME_BUILD_COMMIT");
    let build_commit = env::var("RYFRAME_BUILD_COMMIT")
        .unwrap_or_else(|_| "development".to_owned())
        .trim()
        .to_ascii_lowercase();
    if build_commit != "development"
        && (build_commit.len() != 40 || !build_commit.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err("RYFRAME_BUILD_COMMIT must be a full 40-character Git commit SHA".into());
    }
    println!("cargo:rustc-env=RYFRAME_BUILD_COMMIT={build_commit}");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .ok_or("ryframe-api must be located under the workspace crates directory")?;
    let source_roots = [
        manifest_dir.join("src"),
        workspace_root.join("crates/ryframe-monitor/src"),
    ];

    let mut source_files = Vec::new();
    for root in &source_roots {
        println!("cargo:rerun-if-changed={}", root.display());
        collect_rust_files(root, &mut source_files)?;
    }
    source_files.sort();

    let mut codes = BTreeSet::new();
    let mut capability_bindings = BTreeSet::new();
    for path in source_files {
        println!("cargo:rerun-if-changed={}", path.display());
        let source = fs::read_to_string(&path)?;
        let file = syn::parse_file(&source)
            .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
        let source_label = path
            .strip_prefix(workspace_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        collect_permission_codes(
            &file.items,
            &mut codes,
            &mut capability_bindings,
            &source_label,
            "",
        )?;
    }

    if codes.is_empty() {
        return Err("no #[perm(...)] route permissions were found".into());
    }

    let mut generated = String::from("const ROUTE_PERMISSION_CODES: &[&str] = &[\n");
    for code in codes {
        generated.push_str(&format!("    {code:?},\n"));
    }
    generated.push_str("];\n");
    generated.push_str("const ROUTE_CAPABILITY_BINDINGS: &[RouteCapabilityBinding] = &[\n");
    for (source, handler, method, path, capability, permission) in capability_bindings {
        generated.push_str(&format!(
            "    ({source:?}, {handler:?}, {method:?}, {path:?}, {capability:?}, {permission:?}),\n"
        ));
    }
    generated.push_str("];\n");
    fs::write(
        PathBuf::from(env::var("OUT_DIR")?).join("permission_catalog.rs"),
        generated,
    )?;
    Ok(())
}

fn collect_rust_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rust_files(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

fn collect_permission_codes(
    items: &[Item],
    codes: &mut BTreeSet<String>,
    capability_bindings: &mut BTreeSet<RouteCapabilityBinding>,
    source: &str,
    module_path: &str,
) -> syn::Result<()> {
    for item in items {
        match item {
            Item::Fn(function) => {
                let (permission, capability) =
                    collect_permission_attributes(&function.attrs, codes)?;
                if let Some(capability) = capability {
                    let handler = if module_path.is_empty() {
                        function.sig.ident.to_string()
                    } else {
                        format!("{module_path}::{}", function.sig.ident)
                    };
                    let mut route_found = false;
                    for attribute in &function.attrs {
                        let Some(method) = route_method(attribute) else {
                            continue;
                        };
                        route_found = true;
                        let paths = attribute
                            .parse_args_with(Punctuated::<LitStr, Token![,]>::parse_terminated)?;
                        for path in paths {
                            capability_bindings.insert((
                                source.to_owned(),
                                handler.clone(),
                                method.to_owned(),
                                path.value(),
                                capability.clone(),
                                permission.clone(),
                            ));
                        }
                    }
                    if !route_found {
                        return Err(syn::Error::new_spanned(
                            function,
                            "#[capability] is only valid on an HTTP route handler",
                        ));
                    }
                }
            }
            Item::Mod(module) => {
                let _ = collect_permission_attributes(&module.attrs, codes)?;
                if let Some((_, nested_items)) = &module.content {
                    let nested_path = if module_path.is_empty() {
                        module.ident.to_string()
                    } else {
                        format!("{module_path}::{}", module.ident)
                    };
                    collect_permission_codes(
                        nested_items,
                        codes,
                        capability_bindings,
                        source,
                        &nested_path,
                    )?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn collect_permission_attributes(
    attributes: &[Attribute],
    codes: &mut BTreeSet<String>,
) -> syn::Result<(Option<String>, Option<String>)> {
    let mut permission = None;
    let mut capability = None;
    for attribute in attributes {
        let marker = attribute
            .path()
            .segments
            .last()
            .map(|segment| segment.ident.to_string());
        if marker.as_deref() != Some("perm") && marker.as_deref() != Some("capability") {
            continue;
        }

        let literal = attribute.parse_args::<LitStr>()?;
        let value = literal.value();
        if value.trim() != value || value.is_empty() {
            return Err(syn::Error::new_spanned(
                attribute,
                "route marker must be non-empty and trimmed",
            ));
        }
        if marker.as_deref() == Some("perm") {
            if !value.contains(':') {
                return Err(syn::Error::new_spanned(
                    attribute,
                    "permission code must contain ':'",
                ));
            }
            codes.insert(value.clone());
            permission = Some(value);
        } else {
            if !value.contains('.') {
                return Err(syn::Error::new_spanned(
                    attribute,
                    "capability code must contain '.'",
                ));
            }
            capability = Some(value);
        }
    }
    Ok((permission, capability))
}

fn route_method(attribute: &Attribute) -> Option<&'static str> {
    match attribute.path().segments.last()?.ident.to_string().as_str() {
        "get" => Some("GET"),
        "post" => Some("POST"),
        "put" => Some("PUT"),
        "delete" => Some("DELETE"),
        _ => None,
    }
}

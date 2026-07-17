use std::{
    collections::BTreeSet,
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use syn::{Attribute, Item, LitStr};

fn main() -> Result<(), Box<dyn Error>> {
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
    for path in source_files {
        println!("cargo:rerun-if-changed={}", path.display());
        let source = fs::read_to_string(&path)?;
        let file = syn::parse_file(&source)
            .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
        collect_permission_codes(&file.items, &mut codes)?;
    }

    if codes.is_empty() {
        return Err("no #[perm(...)] route permissions were found".into());
    }

    let mut generated = String::from("const ROUTE_PERMISSION_CODES: &[&str] = &[\n");
    for code in codes {
        generated.push_str(&format!("    {code:?},\n"));
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

fn collect_permission_codes(items: &[Item], codes: &mut BTreeSet<String>) -> syn::Result<()> {
    for item in items {
        match item {
            Item::Fn(function) => collect_permission_attributes(&function.attrs, codes)?,
            Item::Mod(module) => {
                collect_permission_attributes(&module.attrs, codes)?;
                if let Some((_, nested_items)) = &module.content {
                    collect_permission_codes(nested_items, codes)?;
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
) -> syn::Result<()> {
    for attribute in attributes {
        let is_permission = attribute
            .path()
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "perm");
        if !is_permission {
            continue;
        }

        let literal = attribute.parse_args::<LitStr>()?;
        let code = literal.value();
        if code.trim() != code || !code.contains(':') {
            return Err(syn::Error::new_spanned(
                attribute,
                "permission code must be trimmed and contain ':'",
            ));
        }
        codes.insert(code);
    }
    Ok(())
}

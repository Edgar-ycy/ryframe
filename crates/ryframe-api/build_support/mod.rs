mod catalog;
mod model;
mod render;
mod route_source;
mod validation;

use std::{
    collections::BTreeSet,
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use catalog::load_catalog;
use render::render_catalog;
use route_source::{collect_routes, collect_rust_files};
use validation::{append_manual_routes, validate_routes};

pub(super) fn run() -> Result<(), Box<dyn Error>> {
    configure_build_commit()?;

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .ok_or("ryframe-api 必须位于工作区 crates 目录下")?;
    let catalog_root = workspace_root.join("catalog");
    let catalog_path = catalog_root.join("access.toml");
    let generated_catalog_path = catalog_root.join("access.generated.toml");
    println!("cargo:rerun-if-changed={}", catalog_path.display());
    println!(
        "cargo:rerun-if-changed={}",
        generated_catalog_path.display()
    );
    let catalog = load_catalog(&catalog_path, &generated_catalog_path)?;

    let source_root = manifest_dir.join("src");
    println!("cargo:rerun-if-changed={}", source_root.display());
    let mut source_files = Vec::new();
    collect_rust_files(&source_root, &mut source_files)?;
    source_files.sort();

    let mut routes = Vec::new();
    let mut compiled_handlers = BTreeSet::new();
    for path in source_files {
        println!("cargo:rerun-if-changed={}", path.display());
        let source = fs::read_to_string(&path)?;
        let file = syn::parse_file(&source)
            .map_err(|error| format!("无法解析 {}: {error}", path.display()))?;
        let source_label = path
            .strip_prefix(workspace_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        collect_routes(
            &file.items,
            &mut routes,
            &mut compiled_handlers,
            &source_label,
            "",
        )?;
    }

    append_manual_routes(&catalog, &compiled_handlers, &mut routes)?;
    routes.sort_by(|left, right| {
        (
            &left.source,
            &left.handler,
            &left.method,
            &left.path,
            &left.capability,
            &left.permission,
        )
            .cmp(&(
                &right.source,
                &right.handler,
                &right.method,
                &right.path,
                &right.capability,
                &right.permission,
            ))
    });

    let policies = validate_routes(&catalog, &routes)?;
    let generated = render_catalog(&catalog, &routes, &policies);
    fs::write(
        PathBuf::from(env::var("OUT_DIR")?).join("permission_catalog.rs"),
        generated,
    )?;
    Ok(())
}

fn configure_build_commit() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed=RYFRAME_BUILD_COMMIT");
    let build_commit = env::var("RYFRAME_BUILD_COMMIT")
        .unwrap_or_else(|_| "development".to_owned())
        .trim()
        .to_ascii_lowercase();
    if build_commit != "development"
        && (build_commit.len() != 40 || !build_commit.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err("RYFRAME_BUILD_COMMIT 必须是完整的 40 位 Git 提交 SHA".into());
    }
    println!("cargo:rustc-env=RYFRAME_BUILD_COMMIT={build_commit}");
    Ok(())
}

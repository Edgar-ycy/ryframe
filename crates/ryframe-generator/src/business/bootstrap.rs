use std::{
    fs,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::ResourceError;

#[derive(Clone, Debug)]
pub struct BusinessBootstrapOptions<'a> {
    pub workspace_root: &'a Path,
    pub module: &'a str,
    pub write: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BusinessBootstrapReport {
    pub created: Vec<String>,
    pub updated: Vec<String>,
}

struct PlannedFile {
    path: PathBuf,
    content: String,
    created: bool,
}

/// 创建一个可立即接入 API、Worker 和迁移程序的空业务 crate。
///
/// 空 crate 自带受管的零资源 generated 模块，因此组合根在尚未生成第一个资源前也能编译。
pub fn bootstrap_business_package(
    options: BusinessBootstrapOptions<'_>,
) -> Result<BusinessBootstrapReport, ResourceError> {
    validate_module(options.module)?;
    let package = format!("{}-business", options.module);
    let crate_ident = package.replace('-', "_");
    let crate_root = options.workspace_root.join("crates").join(&package);
    if crate_root.exists() {
        return Err(error(
            format!("业务 crate 已存在：{}", crate_root.display()),
            &crate_root,
        ));
    }

    let root_manifest = options.workspace_root.join("Cargo.toml");
    let composition_manifest = options.workspace_root.join("crates/ryframe/Cargo.toml");
    let registry = options
        .workspace_root
        .join("crates/ryframe/src/business.rs");
    let root_source = read(&root_manifest)?;
    let composition_source = read(&composition_manifest)?;
    let registry_source = read(&registry)?;

    let generated = empty_generated_module();
    let mut planned = vec![
        PlannedFile {
            path: crate_root.join("Cargo.toml"),
            content: business_manifest(&package, options.module),
            created: true,
        },
        PlannedFile {
            path: crate_root.join("src/lib.rs"),
            content: business_lib(),
            created: true,
        },
        PlannedFile {
            path: crate_root.join("src/resources/mod.rs"),
            content: resources_module(),
            created: true,
        },
        PlannedFile {
            path: crate_root.join("src/extensions/mod.rs"),
            content: "// 在此实现标准 CRUD 无法表达的领域流程。\n".into(),
            created: true,
        },
        PlannedFile {
            path: crate_root.join("src/generated/mod.rs"),
            content: generated.clone(),
            created: true,
        },
        PlannedFile {
            path: crate_root.join(".ryframe/generated.toml"),
            content: ownership_manifest(&generated),
            created: true,
        },
        PlannedFile {
            path: root_manifest,
            content: add_workspace_member(&root_source, &package)?,
            created: false,
        },
        PlannedFile {
            path: composition_manifest,
            content: add_composition_dependency(&composition_source, &package)?,
            created: false,
        },
        PlannedFile {
            path: registry,
            content: add_module_registration(&registry_source, &crate_ident)?,
            created: false,
        },
    ];
    planned.sort_by(|left, right| left.path.cmp(&right.path));
    let mut report = BusinessBootstrapReport::default();
    for file in &planned {
        let relative = file
            .path
            .strip_prefix(options.workspace_root)
            .unwrap_or(&file.path)
            .to_string_lossy()
            .replace('\\', "/");
        if file.created {
            report.created.push(relative);
        } else if read(&file.path)? != file.content {
            report.updated.push(relative);
        }
    }
    if options.write {
        install(options.workspace_root, &planned)?;
    }
    Ok(report)
}

fn validate_module(module: &str) -> Result<(), ResourceError> {
    if module.is_empty()
        || !module.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        return Err(ResourceError::new(
            "业务模块名只能包含小写字母、数字、中划线和下划线",
            "示例：cargo ryframe new-business order",
        ));
    }
    Ok(())
}

fn read(path: &Path) -> Result<String, ResourceError> {
    fs::read_to_string(path).map_err(|io| error(format!("无法读取文件：{io}"), path))
}

fn add_workspace_member(source: &str, package: &str) -> Result<String, ResourceError> {
    let member = format!("    \"crates/{package}\",\n");
    if source.contains(&format!("\"crates/{package}\"")) {
        return Err(ResourceError::new(
            "业务 crate 已登记为 Workspace 成员",
            "请选择新的模块名",
        ));
    }
    let marker = "    \"xtask\",\n";
    if !source.contains(marker) {
        return Err(ResourceError::new(
            "未找到 Workspace members 插入位置",
            "检查根 Cargo.toml 的 members 配置",
        ));
    }
    Ok(source.replacen(marker, &(member + marker), 1))
}

fn add_composition_dependency(source: &str, package: &str) -> Result<String, ResourceError> {
    let crate_ident = package.replace('-', "_");
    if source.contains(&format!("{package} =")) || source.contains(&format!("{crate_ident} =")) {
        return Err(ResourceError::new(
            "组合根已存在同名业务依赖",
            "请选择新的模块名",
        ));
    }
    let dependency = format!(
        "{package} = {{ path = \"../{package}\", default-features = false, optional = true }}\n"
    );
    if !source.contains("[dev-dependencies]") {
        return Err(ResourceError::new(
            "未找到组合根依赖插入位置",
            "检查 crates/ryframe/Cargo.toml",
        ));
    }
    let mut updated = source.replacen(
        "[dev-dependencies]",
        &(dependency + "\n[dev-dependencies]"),
        1,
    );
    updated = add_feature_items(&updated, "bin-api", &["dep", "api"], package)?;
    updated = add_feature_items(&updated, "bin-worker", &["dep", "runtime"], package)?;
    add_feature_items(&updated, "bin-migrate", &["dep", "migration"], package)
}

fn add_feature_items(
    source: &str,
    feature: &str,
    kinds: &[&str],
    package: &str,
) -> Result<String, ResourceError> {
    let start = format!("{feature} = [\n");
    let start_at = source.find(&start).ok_or_else(|| {
        ResourceError::new(
            format!("未找到 feature {feature}"),
            "检查组合根 feature 配置",
        )
    })?;
    let closing_at = source[start_at + start.len()..]
        .find("]\n")
        .map(|offset| start_at + start.len() + offset)
        .ok_or_else(|| {
            ResourceError::new(
                format!("feature {feature} 缺少结束位置"),
                "检查组合根 feature 配置",
            )
        })?;
    let crate_ident = package.replace('-', "_");
    if source[start_at..closing_at].contains(&crate_ident)
        || source[start_at..closing_at].contains(package)
    {
        return Err(ResourceError::new(
            format!("feature {feature} 已注册该业务 crate"),
            "请选择新的模块名",
        ));
    }
    let lines = kinds
        .iter()
        .map(|kind| match *kind {
            "dep" => format!("    \"dep:{package}\",\n"),
            feature => format!("    \"{package}/{feature}\",\n"),
        })
        .collect::<String>();
    Ok(format!(
        "{}{}{}",
        &source[..closing_at],
        lines,
        &source[closing_at..]
    ))
}

fn add_module_registration(source: &str, crate_ident: &str) -> Result<String, ResourceError> {
    let call = format!("{crate_ident}::module(),");
    if source.contains(&call) {
        return Err(ResourceError::new(
            "业务模块已在组合根注册",
            "请选择新的模块名",
        ));
    }
    if source.contains("    vec![]\n") {
        return Ok(source.replacen(
            "    vec![]\n",
            &format!("    vec![\n        {call}\n    ]\n"),
            1,
        ));
    }
    let start = source
        .find("pub fn business_modules() -> Vec<RyFrameBusinessModule> {\n    vec![\n")
        .ok_or_else(|| {
            ResourceError::new(
                "未找到业务模块注册表",
                "检查 crates/ryframe/src/business.rs",
            )
        })?;
    let insert_at = source[start..]
        .find("    ]\n")
        .map(|offset| start + offset)
        .ok_or_else(|| {
            ResourceError::new(
                "业务模块注册表格式无效",
                "检查 crates/ryframe/src/business.rs",
            )
        })?;
    Ok(format!(
        "{}        {call}\n{}",
        &source[..insert_at],
        &source[insert_at..]
    ))
}

fn business_manifest(package: &str, module: &str) -> String {
    format!(
        "[package]\nname = \"{package}\"\nversion.workspace = true\nedition.workspace = true\nrust-version.workspace = true\nauthors.workspace = true\nlicense.workspace = true\nrepository.workspace = true\n\n[package.metadata.ryframe]\nkind = \"business\"\nmodule = \"{module}\"\n\n[features]\ndefault = [\"api\", \"migration\"]\ncatalog = []\nruntime = [\"ryframe-sdk/runtime\"]\npersistence = [\"runtime\", \"ryframe-sdk/persistence\"]\napi = [\"persistence\", \"ryframe-sdk/api\"]\nmigration = [\"persistence\", \"ryframe-sdk/migration\"]\n\n[dependencies]\nryframe-sdk = {{ path = \"../ryframe-sdk\", default-features = false }}\n\n[lints]\nworkspace = true\n"
    )
}

fn business_lib() -> String {
    "pub mod extensions;\npub mod resources;\n#[cfg(feature = \"runtime\")]\npub mod generated;\n\npub fn resource_descriptors() -> Vec<ryframe_sdk::ResourceDescriptor> {\n    resources::descriptors()\n}\n\n#[cfg(feature = \"runtime\")]\npub fn module() -> ryframe_sdk::RyFrameBusinessModule {\n    let builder = ryframe_sdk::BusinessModuleBuilder::new(env!(\"CARGO_PKG_NAME\").trim_end_matches(\"-business\"))\n        .resources(&generated::RESOURCES);\n    #[cfg(feature = \"api\")]\n    let builder = builder.routes(generated::routes).openapi(generated::openapi);\n    #[cfg(feature = \"migration\")]\n    let builder = builder.migrations(generated::migrations::migrations());\n    builder.build()\n}\n".into()
}

fn resources_module() -> String {
    "use ryframe_sdk::ResourceDescriptor;\n\n/// 返回本业务 crate 已声明的资源描述符。新增资源后在此登记。\npub fn descriptors() -> Vec<ResourceDescriptor> {\n    vec![]\n}\n\n// 在此定义 #[derive(ryframe_sdk::ResourceModel)] 的业务资源。\n".into()
}

fn empty_generated_module() -> String {
    "// @generated by RyFrame。第一个资源生成后会自动替换。\n// 请勿手工修改。\n\npub static RESOURCES: [ryframe_sdk::ResourceDescriptor; 0] = [];\n\n#[cfg(feature = \"api\")]\npub fn routes(_: ryframe_sdk::BusinessRuntimeContext) -> ryframe_sdk::AppResult<ryframe_sdk::axum::Router> {\n    Ok(ryframe_sdk::axum::Router::new())\n}\n\n#[cfg(feature = \"api\")]\npub fn openapi() -> ryframe_sdk::OpenApiDocument {\n    ryframe_sdk::OpenApiDocument::default()\n}\n\n#[cfg(feature = \"migration\")]\npub mod migrations {\n    pub fn migrations() -> Vec<std::sync::Arc<dyn ryframe_sdk::BusinessMigration>> {\n        Vec::new()\n    }\n}\n".into()
}

fn ownership_manifest(content: &str) -> String {
    let hash = hex::encode(Sha256::digest(content.as_bytes()));
    format!(
        "format_version = 1\n\n[[entries]]\nresource = \"__catalog__\"\npath = \"src/generated/mod.rs\"\ncontent_hash = \"{hash}\"\n"
    )
}

fn install(workspace_root: &Path, files: &[PlannedFile]) -> Result<(), ResourceError> {
    let stage = tempfile::Builder::new()
        .prefix(".ryframe-business-stage-")
        .tempdir_in(workspace_root)
        .map_err(|io| error(format!("无法创建暂存目录：{io}"), workspace_root))?;
    for file in files {
        let relative = file
            .path
            .strip_prefix(workspace_root)
            .map_err(|_| error("创建路径超出 Workspace", &file.path))?;
        let staged = stage.path().join(relative);
        if let Some(parent) = staged.parent() {
            fs::create_dir_all(parent)
                .map_err(|io| error(format!("无法创建暂存目录：{io}"), parent))?;
        }
        fs::write(&staged, &file.content)
            .map_err(|io| error(format!("无法写入暂存文件：{io}"), &staged))?;
    }
    let backup = stage.path().join("backup");
    let mut installed = Vec::new();
    let result = (|| {
        for file in files {
            if file.path.exists() {
                let relative = file.path.strip_prefix(workspace_root).expect("已验证路径");
                let saved = backup.join(relative);
                if let Some(parent) = saved.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|io| error(format!("无法创建备份目录：{io}"), parent))?;
                }
                fs::copy(&file.path, &saved)
                    .map_err(|io| error(format!("无法备份目标文件：{io}"), &file.path))?;
            }
        }
        for file in files {
            let relative = file.path.strip_prefix(workspace_root).expect("已验证路径");
            if let Some(parent) = file.path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|io| error(format!("无法创建目标目录：{io}"), parent))?;
            }
            if file.path.exists() {
                fs::remove_file(&file.path)
                    .map_err(|io| error(format!("无法替换目标文件：{io}"), &file.path))?;
            }
            fs::copy(stage.path().join(relative), &file.path)
                .map_err(|io| error(format!("无法安装目标文件：{io}"), &file.path))?;
            installed.push(file.path.clone());
        }
        Ok(())
    })();
    if let Err(failure) = result {
        for path in installed.into_iter().rev() {
            let _ = fs::remove_file(path);
        }
        for file in files.iter().rev() {
            let relative = file.path.strip_prefix(workspace_root).expect("已验证路径");
            let saved = backup.join(relative);
            if saved.exists() {
                let _ = fs::copy(saved, &file.path);
            }
        }
        return Err(failure);
    }
    Ok(())
}

fn error(message: impl Into<String>, path: &Path) -> ResourceError {
    ResourceError::new(
        message,
        "确认 Workspace 结构完整，或使用 --dry-run 查看将执行的改动",
    )
    .with_file(path.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::{BusinessBootstrapOptions, bootstrap_business_package};
    use std::{fs, path::Path};

    fn create_workspace(root: &Path) {
        fs::create_dir_all(root.join("crates/ryframe/src")).expect("创建组合根目录");
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\n    \"crates/ryframe\",\n    \"xtask\",\n]\nresolver = \"3\"\n",
        )
        .expect("写入工作区清单");
        fs::write(
            root.join("crates/ryframe/Cargo.toml"),
            "[package]\nname = \"ryframe\"\n\n[dependencies]\n\n[dev-dependencies]\n\n[features]\nbin-api = [\n]\nbin-worker = [\n]\nbin-migrate = [\n]\n",
        )
        .expect("写入组合根清单");
        fs::write(
            root.join("crates/ryframe/src/business.rs"),
            "use ryframe_sdk::RyFrameBusinessModule;\n\npub fn business_modules() -> Vec<RyFrameBusinessModule> {\n    vec![]\n}\n",
        )
        .expect("写入业务注册表");
    }

    #[test]
    fn bootstrap_creates_crate_and_registers_all_processes() {
        let directory = tempfile::tempdir().expect("创建临时目录");
        create_workspace(directory.path());

        let report = bootstrap_business_package(BusinessBootstrapOptions {
            workspace_root: directory.path(),
            module: "order",
            write: true,
        })
        .expect("创建业务 crate");

        assert_eq!(report.created.len(), 6);
        assert_eq!(report.updated.len(), 3);
        let crate_root = directory.path().join("crates/order-business");
        assert!(crate_root.join("src/generated/mod.rs").is_file());
        assert!(crate_root.join(".ryframe/generated.toml").is_file());
        let manifest = fs::read_to_string(crate_root.join("Cargo.toml")).expect("读取业务清单");
        assert!(manifest.contains("kind = \"business\""));
        let composition = fs::read_to_string(directory.path().join("crates/ryframe/Cargo.toml"))
            .expect("读取组合根清单");
        for expected in [
            "order-business = { path = \"../order-business\"",
            "\"order-business/api\"",
            "\"order-business/runtime\"",
            "\"order-business/migration\"",
        ] {
            assert!(composition.contains(expected), "缺少 {expected}");
        }
        let registry = fs::read_to_string(directory.path().join("crates/ryframe/src/business.rs"))
            .expect("读取业务注册表");
        assert!(registry.contains("order_business::module()"));
    }

    #[test]
    fn dry_run_only_reports_planned_changes() {
        let directory = tempfile::tempdir().expect("创建临时目录");
        create_workspace(directory.path());
        let root_manifest = directory.path().join("Cargo.toml");
        let original = fs::read_to_string(&root_manifest).expect("读取原始清单");

        let report = bootstrap_business_package(BusinessBootstrapOptions {
            workspace_root: directory.path(),
            module: "inventory",
            write: false,
        })
        .expect("预览业务 crate");

        assert_eq!(report.created.len(), 6);
        assert!(!directory.path().join("crates/inventory-business").exists());
        assert_eq!(
            fs::read_to_string(root_manifest).expect("读取清单"),
            original
        );
    }
}

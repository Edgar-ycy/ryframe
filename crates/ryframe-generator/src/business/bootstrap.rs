use std::{fs, path::Path};

use sha2::{Digest, Sha256};

use crate::ResourceError;

mod plan;
mod registration;
use plan::PlannedFile;
use registration::add_module_registration;

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

/// 创建一个可立即接入 API、Worker 和迁移程序的空业务 crate。
///
/// 空 crate 自带受管的零资源 generated 模块，因此组合根在尚未生成第一个资源前也能编译。
pub fn bootstrap_business_package(
    options: BusinessBootstrapOptions<'_>,
) -> Result<BusinessBootstrapReport, ResourceError> {
    validate_module(options.module)?;
    let files = plan::build(options.workspace_root, options.module)?;
    let report = plan::report(options.workspace_root, &files)?;
    if options.write {
        plan::install(options.workspace_root, &files)?;
    }
    Ok(report)
}

fn format_planned(workspace: &Path, files: &mut [PlannedFile]) -> Result<(), ResourceError> {
    let mut assets = files
        .iter_mut()
        .map(|file| crate::GeneratedAsset {
            resource: "__catalog__".into(),
            root: crate::AssetRoot::Backend,
            path: file.path.to_string_lossy().into_owned(),
            content: std::mem::take(&mut file.content),
        })
        .collect::<Vec<_>>();
    crate::resource::rust_assets_in_workspace(workspace, &mut assets)?;
    for (file, asset) in files.iter_mut().zip(assets) {
        file.content = asset.content;
    }
    let generated = files
        .iter()
        .find(|file| file.path.ends_with("src/generated/mod.rs"))
        .expect("初始化计划包含生成模块");
    let ownership = ownership_manifest(&generated.content);
    files
        .iter_mut()
        .find(|file| file.path.ends_with(".ryframe/generated.toml"))
        .expect("初始化计划包含 ownership")
        .content = ownership;
    Ok(())
}

pub(crate) fn validate_module(module: &str) -> Result<(), ResourceError> {
    if !module
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_lowercase)
        || !module.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        return Err(ResourceError::new(
            "业务模块名须以小写字母开头，只包含小写字母、数字、中划线和下划线",
            "示例：cargo generate -- new-business order-business --write",
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
    updated = add_feature_items(&updated, "bin-migrate", &["dep", "migration"], package)?;
    add_feature_items(&updated, "bin-tenant-data", &["dep", "runtime"], package)
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

fn business_manifest(package: &str, module: &str) -> String {
    format!(
        include_str!("manifest.toml.tpl"),
        package = package,
        module = module,
    )
}

fn business_lib() -> String {
    "pub mod extensions;\npub mod resources;\n#[cfg(feature = \"runtime\")]\npub mod generated;\n\npub fn resource_descriptors() -> Vec<ryframe_sdk::ResourceDescriptor> {\n    resources::descriptors()\n}\n\n#[cfg(feature = \"runtime\")]\npub fn module() -> ryframe_sdk::RyFrameBusinessModule {\n    let module = env!(\"CARGO_PKG_NAME\").strip_suffix(\"-business\").expect(\"业务包以 -business 结尾\");\n    let builder = ryframe_sdk::BusinessModuleBuilder::new(module)\n        .resources(generated::RESOURCES.iter())\n        .control_tables(generated::CONTROL_TABLES)\n        .tenant_data_tables(generated::tenant_catalog::TENANT_DATA_TABLES);\n    #[cfg(feature = \"api\")]\n    let builder = builder.routes(generated::routes).openapi(generated::openapi);\n    #[cfg(feature = \"migration\")]\n    let builder = builder.migrations(generated::migrations::migrations());\n    builder.build()\n}\n".into()
}

fn resources_module() -> String {
    "use ryframe_sdk::ResourceDescriptor;\n\n/// 返回本业务 crate 已声明的资源描述符。新增资源后在此登记。\npub fn descriptors() -> Vec<ResourceDescriptor> {\n    vec![]\n}\n\n// 在此定义 #[derive(ryframe_sdk::ResourceModel)] 的业务资源。\n".into()
}

fn empty_generated_module() -> String {
    "// @generated by RyFrame。第一个资源生成后会自动替换。\n// 请勿手工修改。\n\npub mod tenant_catalog {\n    pub static TENANT_DATA_TABLES: &[ryframe_sdk::TenantDataTableDescriptor] = &[];\n}\n\npub static CONTROL_TABLES: &[ryframe_sdk::BusinessControlTableDescriptor] = &[];\n\npub static RESOURCES: [ryframe_sdk::ResourceDescriptor; 0] = [];\n\n#[cfg(feature = \"api\")]\npub fn routes(_: ryframe_sdk::BusinessRuntimeContext) -> ryframe_sdk::AppResult<ryframe_sdk::axum::Router> {\n    Ok(ryframe_sdk::axum::Router::new())\n}\n\n#[cfg(feature = \"api\")]\npub fn openapi() -> ryframe_sdk::OpenApiDocument {\n    ryframe_sdk::OpenApiDocument::default()\n}\n\n#[cfg(feature = \"migration\")]\npub mod migrations {\n    pub fn migrations() -> Vec<std::sync::Arc<dyn ryframe_sdk::BusinessMigration>> {\n        Vec::new()\n    }\n}\n".into()
}

fn ownership_manifest(content: &str) -> String {
    let hash = hex::encode(Sha256::digest(content.as_bytes()));
    format!(
        "format_version = 1\n\n[[entries]]\nresource = \"__catalog__\"\npath = \"src/generated/mod.rs\"\ncontent_hash = \"{hash}\"\n"
    )
}

fn error(message: impl Into<String>, path: &Path) -> ResourceError {
    ResourceError::new(
        message,
        "确认 Workspace 结构完整；省略 --write 预览文件，或使用 --plan 只查看任务",
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
            "[package]\nname = \"ryframe\"\n\n[dependencies]\n\n[dev-dependencies]\n\n[features]\nbin-api = [\n]\nbin-worker = [\n]\nbin-migrate = [\n]\nbin-tenant-data = [\n]\n",
        )
        .expect("写入组合根清单");
        fs::write(
            root.join("crates/ryframe/src/business.rs"),
            "use ryframe_sdk::RyFrameBusinessModule;\n\npub fn business_modules() -> Vec<RyFrameBusinessModule> {\n    vec![]\n}\n",
        )
        .expect("写入业务注册表");
    }

    #[test]
    fn concurrent_registration_edit_is_rejected_before_new_crate_write() {
        let directory = tempfile::tempdir().unwrap();
        create_workspace(directory.path());
        let planned = super::plan::build(directory.path(), "order").unwrap();
        let manifest = directory.path().join("Cargo.toml");
        let edited = fs::read_to_string(&manifest).unwrap() + "\n# 并发手写变更\n";
        fs::write(&manifest, &edited).unwrap();
        let error = super::plan::install(directory.path(), &planned)
            .unwrap_err()
            .to_string();
        assert!(error.contains("compare-and-swap"));
        assert_eq!(fs::read_to_string(&manifest).unwrap(), edited);
        assert!(!directory.path().join("crates/order-business").exists());
    }

    #[test]
    fn concurrently_created_business_file_is_never_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        create_workspace(directory.path());
        let original = fs::read(directory.path().join("Cargo.toml")).unwrap();
        let planned = super::plan::build(directory.path(), "order").unwrap();
        let source = directory.path().join("crates/order-business/src/lib.rs");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "// 并发手写资源\n").unwrap();
        let error = super::plan::install(directory.path(), &planned)
            .unwrap_err()
            .to_string();
        assert!(error.contains("compare-and-swap"));
        assert_eq!(fs::read_to_string(&source).unwrap(), "// 并发手写资源\n");
        assert_eq!(
            fs::read(directory.path().join("Cargo.toml")).unwrap(),
            original
        );
        assert!(
            !directory
                .path()
                .join("crates/order-business/.ryframe/generated.toml")
                .exists()
        );
    }

    #[test]
    fn windows_line_endings_are_preserved_as_preimage_and_normalized_in_output() {
        let directory = tempfile::tempdir().unwrap();
        create_workspace(directory.path());
        for file in [
            "Cargo.toml",
            "crates/ryframe/Cargo.toml",
            "crates/ryframe/src/business.rs",
        ] {
            let path = directory.path().join(file);
            let original = fs::read_to_string(&path).unwrap();
            fs::write(&path, original.replace('\n', "\r\n")).unwrap();
        }
        bootstrap_business_package(BusinessBootstrapOptions {
            workspace_root: directory.path(),
            module: "order",
            write: true,
        })
        .unwrap();
        let manifest = fs::read_to_string(directory.path().join("Cargo.toml")).unwrap();
        assert!(manifest.contains("crates/order-business") && !manifest.contains('\r'));
    }

    #[test]
    fn invalid_module_identifier_fails_without_creating_any_cache() {
        let directory = tempfile::tempdir().unwrap();
        for module in ["", "1order", "-order", "_order", "业务"] {
            let error = bootstrap_business_package(BusinessBootstrapOptions {
                workspace_root: directory.path(),
                module,
                write: false,
            })
            .unwrap_err()
            .to_string();
            assert!(error.contains("小写字母开头"));
        }
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
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
        let parsed: toml::Value = toml::from_str(&manifest).expect("业务清单应为合法 TOML");
        for dependency in ["sea-orm", "serde", "utoipa", "validator"] {
            assert_eq!(
                parsed["dependencies"][dependency]["workspace"].as_bool(),
                Some(true)
            );
            assert_eq!(
                parsed["dependencies"][dependency]["optional"].as_bool(),
                Some(true)
            );
        }
        let runtime = parsed["features"]["runtime"]
            .as_array()
            .expect("应提供 runtime feature");
        assert!(!runtime.iter().any(|value| {
            value
                .as_str()
                .is_some_and(|value| value.starts_with("dep:"))
        }));
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
        let parsed: toml::Value = toml::from_str(&composition).expect("组合根清单应为合法 TOML");
        assert_eq!(
            parsed["features"]["bin-tenant-data"],
            toml::Value::Array(vec![
                toml::Value::String("dep:order-business".into()),
                toml::Value::String("order-business/runtime".into()),
            ])
        );
        let registry = fs::read_to_string(directory.path().join("crates/ryframe/src/business.rs"))
            .expect("读取业务注册表");
        assert!(registry.contains("order_business::module()"));
    }

    #[test]
    fn bootstrap_registers_a_second_crate_after_formatting() {
        let directory = tempfile::tempdir().expect("创建临时目录");
        create_workspace(directory.path());
        for module in ["order", "inventory"] {
            bootstrap_business_package(BusinessBootstrapOptions {
                workspace_root: directory.path(),
                module,
                write: true,
            })
            .expect("格式化后的注册表应能继续添加业务 crate");
        }
        let source =
            fs::read_to_string(directory.path().join("crates/ryframe/src/business.rs")).unwrap();
        assert_eq!(source.matches("order_business::module()").count(), 1);
        assert_eq!(source.matches("inventory_business::module()").count(), 1);
        syn::parse_file(&source).unwrap();
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

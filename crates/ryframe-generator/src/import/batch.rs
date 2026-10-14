use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use super::{
    TableInfo,
    model::{pascal_case, rust_model_source_named},
};
use crate::{
    ResourceError,
    business::writer::{BusinessAsset, staged::apply_files},
    resource::write_transaction::{ExpectedFile, verify_expected_file},
};

mod registration;

/// 整批导入的模型与注册计划；表结构读取与文件写入保持分离。
pub struct ImportModelPlan {
    root: std::path::PathBuf,
    assets: Vec<BusinessAsset>,
    expected: BTreeMap<String, ExpectedFile>,
    pub created: Vec<String>,
    pub updated: Vec<String>,
    pub unchanged: Vec<String>,
}

/// 预检全部模型、名称和文件；保留原始结构元数据，不访问数据库、不修改源码。
pub fn plan_import_models(
    workspace_root: &Path,
    business_root: &Path,
    tables: &[TableInfo],
    database: &str,
    strip_prefix: Option<&str>,
) -> Result<ImportModelPlan, ResourceError> {
    if tables.is_empty() {
        return Err(error("没有选中任何表"));
    }
    let mut models = Vec::new();
    let mut assets = Vec::new();
    let mut names = BTreeSet::new();
    let mut types = BTreeSet::new();
    for table in tables {
        let name = strip_prefix
            .filter(|prefix| !prefix.is_empty())
            .and_then(|prefix| table.table_name.strip_prefix(prefix))
            .unwrap_or(&table.table_name);
        let source = rust_model_source_named(table, database, name).map_err(error)?;
        let model_type = pascal_case(name).map_err(error)?;
        if !names.insert(name.to_ascii_lowercase()) || !types.insert(model_type.clone()) {
            return Err(error(format!(
                "批量导入资源名或模型名冲突：表 {} → {name}/{model_type}；调整表映射或前缀",
                table.table_name
            )));
        }
        models.push((name.to_owned(), model_type));
        assets.push(BusinessAsset::new(
            name,
            format!("src/resources/{name}.rs"),
            source,
        ));
        assets.push(BusinessAsset::new(
            name,
            format!(".ryframe/imports/{name}.json"),
            serde_json::to_string_pretty(table)
                .map_err(|serialize| error(format!("无法保存表结构来源：{serialize}")))?
                + "\n",
        ));
    }
    let registry_path = business_root.join("src/resources/mod.rs");
    let original = fs::read_to_string(&registry_path).map_err(|io| {
        error(format!(
            "无法读取描述符注册表 {}：{io}",
            registry_path.display()
        ))
    })?;
    let registry = registration::register(&original, &models)?;
    assets.push(BusinessAsset::new(
        "__catalog__",
        "src/resources/mod.rs",
        registry,
    ));
    format_sources(workspace_root, &mut assets)?;
    let mut plan = ImportModelPlan {
        root: business_root.into(),
        assets: Vec::new(),
        expected: BTreeMap::new(),
        created: Vec::new(),
        updated: Vec::new(),
        unchanged: Vec::new(),
    };
    for asset in assets {
        preflight_asset(&mut plan, asset, &original)?;
    }
    Ok(plan)
}

impl ImportModelPlan {
    /// 返回尚未写入的 Rust 模型和注册内容，便于 CLI 与 IDE 审阅。
    pub fn rust_sources(&self) -> impl Iterator<Item = (&str, &str)> {
        self.assets
            .iter()
            .filter(|asset| asset.path.ends_with(".rs"))
            .map(|asset| (asset.path.as_str(), asset.content.as_str()))
    }

    /// 模型是后续手写事实源，不加入生成产物 ownership。提交全部文件前再次核验前像。
    pub fn write(self) -> Result<(), ResourceError> {
        for (path, expected) in &self.expected {
            verify_expected_file(&self.root.join(path), expected)?;
        }
        if self.assets.is_empty() {
            return Ok(());
        }
        apply_files(&self.root, &self.assets, &[], &self.expected)
    }
}

fn preflight_asset(
    plan: &mut ImportModelPlan,
    asset: BusinessAsset,
    original: &str,
) -> Result<(), ResourceError> {
    let path = plan.root.join(&asset.path);
    match fs::read(&path) {
        Ok(bytes) => {
            plan.expected
                .insert(asset.path.clone(), ExpectedFile::ExactBytes(bytes.clone()));
            if bytes == asset.content.as_bytes() {
                plan.unchanged.push(asset.path);
            } else if asset.path == "src/resources/mod.rs" && bytes == original.as_bytes() {
                plan.updated.push(asset.path.clone());
                plan.assets.push(asset);
            } else {
                return Err(error(format!(
                    "导入目标 {} 已存在且内容不同；保留手写模型，使用 resource 生成；结构变化需显式审阅迁移，本批未写入",
                    path.display()
                )));
            }
        }
        Err(io) if io.kind() == std::io::ErrorKind::NotFound => {
            plan.expected
                .insert(asset.path.clone(), ExpectedFile::Absent);
            plan.created.push(asset.path.clone());
            plan.assets.push(asset);
        }
        Err(io) => {
            return Err(error(format!(
                "无法预检 {}：{io}；本批未写入",
                path.display()
            )));
        }
    }
    Ok(())
}

fn format_sources(root: &Path, assets: &mut [BusinessAsset]) -> Result<(), ResourceError> {
    let mut generated = assets
        .iter_mut()
        .map(|asset| crate::GeneratedAsset {
            resource: asset.resource.clone(),
            root: crate::AssetRoot::Backend,
            path: asset.path.clone(),
            content: std::mem::take(&mut asset.content),
        })
        .collect::<Vec<_>>();
    crate::resource::rust_assets_in_workspace(root, &mut generated)?;
    for (asset, formatted) in assets.iter_mut().zip(generated) {
        asset.content = formatted.content;
    }
    Ok(())
}

pub(super) fn error(message: impl Into<String>) -> ResourceError {
    ResourceError::new(
        message,
        "检查整批表和 src/resources 描述符注册表后重新预览；导入不修改数据库",
    )
}

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use super::super::GenerationPlan;
use crate::{
    business::{
        BusinessGenerateOptions, BusinessGenerateReport, generate_business_package,
        locate_business_package,
    },
    import::{TableInfo, plan_import_models},
};

pub(super) fn generate(
    plan: &GenerationPlan,
    candidate: &Path,
    tables: Option<&[TableInfo]>,
    target: &Path,
) -> Result<BTreeSet<PathBuf>, String> {
    let package =
        locate_business_package(candidate, &plan.package).map_err(|error| error.to_string())?;
    let prefix = package
        .root
        .strip_prefix(&package.workspace_root)
        .map_err(|_| "业务 crate 必须位于当前 Workspace 内；本批未安装")?;
    let mut allowed = BTreeSet::new();
    if let Some(tables) = tables {
        let models = plan_import_models(
            candidate,
            &package.root,
            tables,
            &plan.database,
            plan.strip_table_prefix.as_deref(),
        )
        .map_err(|error| error.to_string())?;
        allowed.extend(
            models
                .created
                .iter()
                .chain(&models.updated)
                .map(|path| prefix.join(path)),
        );
        if !plan.write {
            for (path, content) in models.rust_sources() {
                println!("候选模型 {path}\n{content}");
            }
        }
        models.write().map_err(|error| error.to_string())?;
    }
    let report = generate_business_package(BusinessGenerateOptions {
        current_dir: candidate,
        package: &plan.package,
        models: &plan.models,
        write: true,
        catalog_target: Some(target),
    })
    .map_err(|error| format!("候选业务生成失败：{error}；正式模型、注册和生成目录均未安装"))?;
    allow_report(&mut allowed, prefix, &report);
    allowed.insert(prefix.join(".ryframe/generated.toml"));
    if let Some(tables) = tables {
        verify_imported_schema(&package.root, tables)?;
    }
    Ok(allowed)
}

fn verify_imported_schema(package: &Path, tables: &[TableInfo]) -> Result<(), String> {
    let path = package.join("src/generated/migrations/history.json");
    let history: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&path).map_err(|error| format!("无法读取候选迁移结构：{error}"))?,
    )
    .map_err(|error| error.to_string())?;
    let resources = history
        .get("resources")
        .and_then(serde_json::Value::as_array)
        .ok_or("候选迁移历史缺少资源")?;
    for table in tables {
        let resource = resources
            .iter()
            .find(|resource| {
                resource
                    .pointer("/descriptor/table")
                    .and_then(serde_json::Value::as_str)
                    == Some(table.table_name.as_str())
            })
            .ok_or_else(|| format!("候选迁移未登记表 {}；本批未安装", table.table_name))?;
        let generated = resource
            .get("versions")
            .and_then(serde_json::Value::as_array)
            .and_then(|versions| versions.last())
            .and_then(|version| version.get("canonical"))
            .and_then(serde_json::Value::as_str)
            .ok_or("候选迁移缺少末态结构")?;
        if generated != table.schema_canonical {
            #[cfg(feature = "schema-import")]
            let detail =
                ryframe_db::schema::describe_schema_difference(&table.schema_canonical, generated);
            #[cfg(not(feature = "schema-import"))]
            let detail = "列、索引或约束与读取的结构不一致";
            return Err(format!(
                "表 {} 导入存在未表达的结构差异：{detail}；本批未安装。审阅 .ryframe/imports 来源，使用明确的模型/迁移扩展处理不支持的约束，不跳过接管校验",
                table.table_name
            ));
        }
    }
    Ok(())
}

pub(super) fn allow_report(
    allowed: &mut BTreeSet<PathBuf>,
    prefix: &Path,
    report: &BusinessGenerateReport,
) {
    allowed.extend(
        report
            .created
            .iter()
            .chain(&report.updated)
            .chain(&report.removed)
            .map(|path| prefix.join(path)),
    );
}

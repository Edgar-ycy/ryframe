//! 将启动选择绑定到目标计划的双端来源、构建收据与实际产物。
use std::path::Path;

use serde_json::{Value, json};

use super::super::{
    build,
    dataset_authority::{b0 as b0_backend, check},
    registration::protocol_path,
    verify::{Frontend, backend_sources},
};
use crate::{
    Result,
    cli::RuntimeSources,
    recovery::{
        frontend_build::b0,
        inputs::document::{self, Document},
    },
};

pub(super) fn read(
    coordinator: &Path,
    selected: &RuntimeSources,
    execution: &Value,
    backend: &Document,
    frontend: &Document,
    authority: &Value,
    environment: &Value,
) -> Result<Value> {
    let roots = bind(selected, execution, backend, frontend, authority)?;
    build::source::repository(coordinator)?;
    let before = backend_sources(coordinator, selected, backend, authority, environment)?;
    let proof = Frontend::capture(&selected.source_frontend, frontend, authority)?;
    check(
        proof.inventory()["source"]["snapshot"]["head"] == authority["frontend_sha"],
        "恢复运行前端来源不同于登记的精确提交",
    )?;
    check(
        backend_sources(coordinator, selected, backend, authority, environment)? == before,
        "恢复启动核验期间后端源码、构建上下文或产物发生变化",
    )?;
    proof.verify()?;
    backend.assert_unchanged()?;
    frontend.assert_unchanged()?;
    Ok(roots)
}

fn bind(
    selected: &RuntimeSources,
    execution: &Value,
    backend: &Document,
    frontend: &Document,
    authority: &Value,
) -> Result<Value> {
    let product = selected
        .product_backend
        .as_deref()
        .unwrap_or(&selected.source_backend);
    let adapter = json!(selected.adapter_contract);
    let registered_adapter = &execution["adapter"];
    check(
        authority["backend_adapter_contract"] == adapter
            && if adapter.is_null() {
                registered_adapter.is_null()
            } else {
                registered_adapter["contract"] == adapter
            },
        "恢复启动适配合同与目标计划或显式选择不同",
    )?;
    let b0 = b0_backend::matches(authority);
    check(
        if b0 {
            selected.product_backend.is_some() && product != selected.source_backend.as_path()
        } else {
            adapter.is_null()
                && selected.product_backend.is_none()
                && authority["backend_product_sha"] == authority["backend_execution_sha"]
                && authority["frontend_sha"] != b0::HEAD
        },
        "恢复启动必须使用相同产品与执行来源或精确登记的 B0 适配组合",
    )?;
    check(
        [
            "backend_product_sha",
            "backend_execution_sha",
            "frontend_sha",
        ]
        .into_iter()
        .all(|key| execution[key] == authority[key]),
        "恢复启动提交与目标计划不同",
    )?;
    let roots = json!({
        "source_backend":protocol_path(product)?,
        "execution_backend":protocol_path(&selected.source_backend)?,
        "frontend":protocol_path(&selected.source_frontend)?,
    });
    check(
        execution["roots"] == roots,
        "恢复启动显式来源根与目标计划不同",
    )?;
    check(
        execution["builds"]
            == json!({"backend":document::binding(backend),"frontend":document::binding(frontend)}),
        "恢复启动构建收据未绑定目标计划的两个原始文件",
    )?;
    Ok(
        json!({"backend_product":roots["source_backend"],"backend_execution":roots["execution_backend"],"frontend":roots["frontend"]}),
    )
}

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/runtime/start/sources.rs"]
mod tests;

//! 同一运行、目标、构建和 DataVerified 记录之间的静态交叉关系。
use super::{check, inputs::Inputs, path, request_shape};
use crate::{
    Result,
    recovery::{
        clone::native::json as strict,
        inputs::{
            document::{self, Document},
            product_record::product_hash,
        },
    },
};
use serde_json::{Value, json};

pub(in crate::recovery::runtime) struct RuntimeDocuments {
    pub(in crate::recovery::runtime) bindings: Document,
    pub(in crate::recovery::runtime) backend: Document,
    pub(in crate::recovery::runtime) frontend: Document,
    pub(in crate::recovery::runtime) launch: Document,
}

pub(in crate::recovery::runtime) fn read(
    inputs: &mut Inputs,
    runtime: &Value,
) -> Result<RuntimeDocuments> {
    let documents = read_documents(inputs, runtime)?;
    check(
        super::commands::same_path(
            &documents.launch.path,
            &path(&runtime["paths"]["runtime_dir"])?.join("runtime-launch.json"),
        )?,
        "恢复运行启动收据路径不属于同代目录",
    )?;
    Ok(documents)
}

pub(in crate::recovery::runtime) fn read_documents(
    inputs: &mut Inputs,
    runtime: &Value,
) -> Result<RuntimeDocuments> {
    let mut read = |key: &str| -> Result<Document> {
        let document = inputs.read(&path(&runtime["paths"][key])?)?;
        check(
            runtime["digests"][key] == document.sha256(),
            "恢复运行输入摘要漂移",
        )?;
        Ok(document)
    };
    let bindings = read("bindings")?;
    let backend = read("backend_build")?;
    let frontend = read("frontend_build")?;
    let launch = read("launch")?;
    check(
        backend.value == runtime["backend"] && frontend.value == runtime["frontend"],
        "恢复运行内嵌构建与原始文件不同",
    )?;
    Ok(RuntimeDocuments {
        bindings,
        backend,
        frontend,
        launch,
    })
}

/// 从同一已核验目标和备份还原启动及验证共用的 v2 权威。
pub(in crate::recovery::runtime) fn start_authority(
    bindings: &Value,
    target: &Value,
    reference: &Value,
    manifest: &Value,
) -> Result<Value> {
    strict::exact(bindings, &["record", "manifest"], "恢复 bindings")?;
    let record = &bindings["record"];
    let product = &target["product_plan"];
    check(
        record["status"] == "data_verified"
            && record["plan"] == *product
            && bindings["manifest"] == *manifest
            && product["backup_id"] == manifest["id"],
        "恢复 bindings 未绑定同一已核验产品计划及唯一备份",
    )?;
    check(
        record["plan_hash"] == product_hash(product)?,
        "恢复记录计划摘要与已核验产品计划不同",
    )?;
    check(
        target["product_plan_sha256"] == document::hash(product)?,
        "目标计划的产品文档摘要不同",
    )?;
    let execution = &target["product_execution"];
    check(
        execution["frontend_sha"] == product["frontend_sha"],
        "恢复前端执行源码与已核验产品计划不同",
    )?;
    let authority = json!({
        "format_version": 2,
        "kind": "restore-runtime-authority",
        "restore_id": product["id"],
        "backup_id": product["backup_id"],
        "scope_id": product["scope_id"],
        "plan_hash": record["plan_hash"],
        "data_verified_at": record["data_verified_at"],
        "backup_source_sha": manifest["source_sha"],
        "backend_product_sha": execution["backend_product_sha"],
        "backend_execution_sha": execution["backend_execution_sha"],
        "frontend_sha": execution["frontend_sha"],
        "backend_adapter_contract": execution["adapter"]["contract"],
        "api_endpoint": product["api_ready_url"],
        "worker_endpoint": product["worker_ready_url"],
        "frontend_endpoint": reference["target"]["frontend_url"],
    });
    request_shape::authority(&authority)?;
    Ok(authority)
}

pub(in crate::recovery::runtime) fn verify(
    runtime: &Value,
    target: &Value,
    reference: &Value,
    manifest: &Value,
    documents: &RuntimeDocuments,
) -> Result<Value> {
    let authority = start_authority(&documents.bindings.value, target, reference, manifest)?;
    verify_runtime_projection(runtime, &authority)?;
    let execution = &target["product_execution"];
    let roots = json!({"backend_product":execution["roots"]["source_backend"], "backend_execution":execution["roots"]["execution_backend"], "frontend":execution["roots"]["frontend"]});
    for (key, root) in [
        ("backend_product_root", "backend_product"),
        ("backend_execution_root", "backend_execution"),
        ("frontend_root", "frontend"),
    ] {
        check(
            super::commands::same_path(&path(&runtime["paths"][key])?, &path(&roots[root])?)?,
            "恢复运行产品路径与目标来源不同",
        )?;
    }
    check(
        execution["builds"]
            == json!({"backend":document::binding(&documents.backend), "frontend":document::binding(&documents.frontend)}),
        "恢复构建未绑定目标计划的产品执行来源",
    )?;
    launch(runtime, &documents.launch.value, &roots, &authority)?;
    Ok(authority)
}

fn verify_runtime_projection(runtime: &Value, authority: &Value) -> Result<()> {
    let restore = json!({
        "id": authority["restore_id"],
        "backup_id": authority["backup_id"],
        "scope_id": authority["scope_id"],
        "plan_hash": authority["plan_hash"],
        "data_verified_at": authority["data_verified_at"],
    });
    let source = json!({
        "backup_source_sha": authority["backup_source_sha"],
        "backend_product_sha": authority["backend_product_sha"],
        "backend_execution_sha": authority["backend_execution_sha"],
        "frontend_sha": authority["frontend_sha"],
        "backend_adapter_contract": authority["backend_adapter_contract"],
    });
    let endpoints = json!({
        "api": authority["api_endpoint"],
        "worker": authority["worker_endpoint"],
        "frontend": authority["frontend_endpoint"],
    });
    check(
        runtime["restore"] == restore
            && runtime["source"] == source
            && runtime["endpoints"] == endpoints,
        "恢复运行身份、来源或端点与目标权威不同",
    )
}

fn launch(runtime: &Value, launch: &Value, roots: &Value, authority: &Value) -> Result<()> {
    let request = &launch["request"];
    let mut paths = json!({});
    let mut digests = json!({});
    for key in ["bindings", "backend_build", "frontend_build"] {
        paths[key] = runtime["paths"][key].clone();
        digests[key] = runtime["digests"][key].clone();
    }
    check(
        request["authority"] == *authority
            && request["roots"] == *roots
            && request["paths"] == paths
            && request["digests"] == digests
            && request["artifacts"] == runtime["backend"]["artifacts"],
        "恢复启动请求来源、构建或权威不同",
    )?;
    for role in ["api", "worker", "frontend"] {
        let process = &launch["processes"][role]["process_receipt"];
        check(
            runtime["processes"][role]
                == json!({"receipt_path":process["path"], "receipt_sha256":process["sha256"], "identity":process["identity"]}),
            "恢复运行进程与 launch 绑定不同",
        )?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/runtime/dataset_authority/cross.rs"]
mod tests;

//! 同一运行、目标、构建和 DataVerified 记录之间的静态交叉关系。
use super::{check, inputs::Inputs, path};
use crate::{
    Result,
    recovery::{
        clone::native::json as strict,
        inputs::document::{self, Document},
    },
};
use serde_json::{Value, json};

pub(super) struct RuntimeDocuments {
    pub(super) bindings: Document,
    pub(super) backend: Document,
    pub(super) frontend: Document,
    pub(super) launch: Document,
}

pub(super) fn read(inputs: &mut Inputs, runtime: &Value) -> Result<RuntimeDocuments> {
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
    check(
        launch.path == path(&runtime["paths"]["runtime_dir"])?.join("runtime-launch.json"),
        "恢复运行启动收据路径不属于同代目录",
    )?;
    Ok(RuntimeDocuments {
        bindings,
        backend,
        frontend,
        launch,
    })
}

pub(super) fn verify(
    runtime: &Value,
    target: &Value,
    reference: &Value,
    manifest: &Value,
    documents: &RuntimeDocuments,
) -> Result<Value> {
    let bindings = &documents.bindings.value;
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
    document::digest(&record["plan_hash"], "恢复记录计划摘要")?;
    crate::recovery::inputs::domain::timestamp(&record["data_verified_at"], "恢复数据核验时间")?;
    let execution = &target["product_execution"];
    let source = json!({"backup_source_sha":manifest["source_sha"], "backend_product_sha":execution["backend_product_sha"], "backend_execution_sha":execution["backend_execution_sha"], "frontend_sha":execution["frontend_sha"], "backend_adapter_contract":execution["adapter"].get("contract").unwrap_or(&Value::Null)});
    let endpoints = json!({"api":product["api_ready_url"], "worker":product["worker_ready_url"], "frontend":reference["target"]["frontend_url"]});
    let restore = json!({"id":product["id"], "backup_id":product["backup_id"], "scope_id":product["scope_id"], "plan_hash":record["plan_hash"], "data_verified_at":record["data_verified_at"]});
    check(
        runtime["restore"] == restore
            && runtime["source"] == source
            && runtime["endpoints"] == endpoints,
        "恢复运行身份、来源或端点与目标权威不同",
    )?;
    let roots = json!({"backend_product":execution["roots"]["source_backend"], "backend_execution":execution["roots"]["execution_backend"], "frontend":execution["roots"]["frontend"]});
    for (key, root) in [
        ("backend_product_root", "backend_product"),
        ("backend_execution_root", "backend_execution"),
        ("frontend_root", "frontend"),
    ] {
        check(
            runtime["paths"][key] == roots[root],
            "恢复运行产品路径与目标来源不同",
        )?;
    }
    check(
        execution["builds"]
            == json!({"backend":document::binding(&documents.backend), "frontend":document::binding(&documents.frontend)}),
        "恢复构建未绑定目标计划的产品执行来源",
    )?;
    launch(
        runtime,
        &documents.launch.value,
        &roots,
        &source,
        &restore,
        &endpoints,
    )
}

fn launch(
    runtime: &Value,
    launch: &Value,
    roots: &Value,
    source: &Value,
    restore: &Value,
    endpoints: &Value,
) -> Result<Value> {
    let mut authority = source.clone();
    authority["format_version"] = json!(2);
    authority["kind"] = json!("restore-runtime-authority");
    for (key, input) in [
        ("restore_id", "id"),
        ("backup_id", "backup_id"),
        ("scope_id", "scope_id"),
        ("plan_hash", "plan_hash"),
        ("data_verified_at", "data_verified_at"),
    ] {
        authority[key] = restore[input].clone();
    }
    for role in ["api", "worker", "frontend"] {
        authority[format!("{role}_endpoint")] = endpoints[role].clone();
    }
    let request = &launch["request"];
    let mut paths = json!({});
    let mut digests = json!({});
    for key in ["bindings", "backend_build", "frontend_build"] {
        paths[key] = runtime["paths"][key].clone();
        digests[key] = runtime["digests"][key].clone();
    }
    check(
        request["authority"] == authority
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
    Ok(authority)
}

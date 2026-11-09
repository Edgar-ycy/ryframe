//! 比较原重启构建收据与可观察的执行上下文，保持来源绑定失败与构建阻塞的区别。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::{
        clone::native::{bridge::build_context, json as document, maintenance, resources::common},
        monitoring::execution::ProtectedInputs,
        source::evidence::binding,
    },
};

pub(super) fn observe(backend: &Path, facts: &Value) -> Result<Value> {
    let request = &facts["request"];
    let execution = binding::path(&request["execution_backend"])?;
    let expected_head = document::text(&request["expected_backend_sha"], "原构建源码 SHA")?;
    let execution_root = common::path_text(&execution, "原构建执行根")?;
    let product_binding = &request["backend_build"];
    let maintenance_binding = &request["maintenance_build"];
    let bind = || -> Result<[PathBuf; 2]> {
        Ok([
            common::bound_file(backend, product_binding, "重启原后端构建")?,
            common::bound_file(&execution, maintenance_binding, "重启原维护构建")?,
        ])
    };
    let [product, maintenance_file] = bind()?;
    let protected = ProtectedInputs::open([product.clone(), maintenance_file.clone()])?;
    let result = (|| {
        bind()?;
        let product_observed = match document::read(&product) {
            Ok(receipt) => {
                let build = receipt.get("build").ok_or("原后端构建收据缺少 build")?;
                build_context::current(&execution).map(|current| *build == current)
            }
            Err(error) => Err(error),
        };
        let maintenance_observed =
            match maintenance::verify_evidence(&execution, &maintenance_file, None) {
                Ok(receipt) => {
                    let head = receipt
                        .pointer("/source/snapshot/head")
                        .ok_or("原维护构建收据缺少来源 HEAD")?;
                    Ok(head.as_str() == Some(expected_head)
                        && receipt["backend_root"] == execution_root)
                }
                Err(error) => Err(error),
            };
        Ok(json!({
            "backend": row(product_binding, product_observed),
            "maintenance": row(maintenance_binding, maintenance_observed),
        }))
    })();
    protected.finish(result)
}

fn row(evidence: &Value, observed: Result<bool>) -> Value {
    match observed {
        Ok(matches) => json!({"matches": matches, "evidence": evidence}),
        Err(_) => json!({"matches": false, "error_type": "ValueError", "evidence": evidence}),
    }
}

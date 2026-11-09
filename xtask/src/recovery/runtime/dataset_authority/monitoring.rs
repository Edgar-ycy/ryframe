//! 从同一正式备份、目标与存活代次推导监控预检权威。
use std::path::Path;

use serde_json::{Value, json};

use super::{inputs::Inputs, reads::Window, verified_target::VerifiedTarget};
use crate::{Result, recovery::inputs::document};

pub(in crate::recovery) fn verify(
    backend: &Path,
    runtime_path: &Path,
    target_path: &Path,
) -> Result<Value> {
    let window = Window::begin()?;
    let mut inputs = Inputs::new(backend);
    let result = (|| {
        let verified = VerifiedTarget::read(backend, runtime_path, target_path, &mut inputs)?;
        verified.verify_live(backend, &window)?;
        authority_result(&verified)
    })();
    window.finish(inputs.finish(result))
}

fn authority_result(verified: &VerifiedTarget) -> Result<Value> {
    let value = &verified.runtime.value;
    let launch = &verified.documents.launch;
    let endpoints = &value["endpoints"];
    let origin = |key| -> Result<&str> {
        document::text(&endpoints[key], "监控就绪端点")?
            .strip_suffix("/readyz")
            .ok_or_else(|| "监控就绪端点缺少精确路径".into())
    };
    let endpoints = json!({
        "api_ready":endpoints["api"], "worker_ready":endpoints["worker"],
        "frontend":endpoints["frontend"],
        "api_metrics":format!("{}/api/v1/monitor/metrics", origin("api")?),
        "worker_metrics":format!("{}/metrics", origin("worker")?),
    });
    Ok(json!({
        "format_version":1, "kind":"restore-monitoring-authority",
        "runtime":document::binding(&verified.runtime), "target_plan":document::binding(&verified.target),
        "launch":document::binding(launch), "environment":launch.value["request"]["environment"]["document"],
        "restore":value["restore"], "source":value["source"], "endpoints":endpoints,
        "maintenance_execution":verified.value["maintenance_execution"],
        "product_execution":verified.value["product_execution"],
        "processes":{"api":value["processes"]["api"]["identity"],
            "worker":value["processes"]["worker"]["identity"], "frontend":value["processes"]["frontend"]["identity"]},
        "source_generation":verified.lineage["source_generation"], "dataset_lineage":verified.lineage["dataset_lineage"],
    }))
}

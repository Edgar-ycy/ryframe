//! 从唯一已验证备份抽取 STOP、START、运行收据的同一数据血缘。
use super::{check, inputs::Inputs};
use crate::{
    Result,
    recovery::{
        clone::native::{
            json as strict,
            seed::generation::coordinator::receipts::{RESULT_FIELDS, START_FIELDS},
        },
        inputs::document,
    },
};
use serde_json::{Value, json};

pub(super) fn extract(inputs: &mut Inputs, backup: &Value) -> Result<Value> {
    let stop = inputs.bound(&backup["source_generation"])?;
    let value = &stop.value;
    strict::exact(value, RESULT_FIELDS, "备份停止代次")?;
    let start = inputs.bound(&value["start"])?;
    let runtime = inputs.bound(&value["source_runtime"])?;
    let lineage = inputs.bound(&value["dataset_lineage"])?;
    strict::exact(&start.value, START_FIELDS, "备份启动代次")?;
    let mut fields = vec![
        "format_version",
        "kind",
        "status",
        "source_generation",
        "dataset_lineage",
        "before",
        "after",
        "producer",
        "verification",
        "login_audit",
        "cache_cleanup",
        "evidence",
        "write_effects",
        "started_at",
        "verified_at",
        "remote_writes",
        "restore_qualified",
    ];
    if runtime.value.get("recovery").is_some() {
        fields.push("recovery");
        inputs.bound(&runtime.value["recovery"])?;
    }
    strict::exact(&runtime.value, &fields, "备份来源运行验收")?;
    check(
        backup["source_export"]["source_generation"] == document::binding(&stop)
            && value["status"] == "seed_source_generation_published"
            && start.value["status"] == "seed_source_generation_running"
            && runtime.value["source_generation"] == document::binding(&start)
            && [value, &start.value, &runtime.value]
                .iter()
                .all(|item| item["dataset_lineage"] == document::binding(&lineage)),
        "备份停止、启动、运行验收和数据血缘不是同一不可变来源",
    )?;
    Ok(
        json!({"source_generation":document::binding(&stop), "dataset_lineage":document::binding(&lineage)}),
    )
}

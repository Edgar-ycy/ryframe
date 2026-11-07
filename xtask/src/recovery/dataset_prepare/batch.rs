use super::{
    documents,
    model::{self, Inputs},
};
use crate::Result;
use crate::recovery::clone::native::export::transport::{SystemTransport, Transport};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};
#[path = "batch/client.rs"]
mod client;
#[path = "batch/input.rs"]
mod input;
#[path = "batch/sql.rs"]
mod sql;

pub(super) fn run(inputs: &Inputs, directory: &Path) -> Result<Value> {
    use crate::recovery::clone::native::resources::common;
    let plan_binding = common::binding(&inputs.plan_path)?;
    let plan_identity =
        common::device_inode_for(&inputs.plan_path, &std::fs::metadata(&inputs.plan_path)?)?;
    run_with(
        &inputs.plan,
        &inputs.root,
        directory,
        &SystemTransport,
        || {
            inputs.unchanged()?;
            if common::binding(&inputs.plan_path)? != plan_binding
                || common::device_inode_for(
                    &inputs.plan_path,
                    &std::fs::metadata(&inputs.plan_path)?,
                )? != plan_identity
            {
                return Err("岗位批次原计划身份或内容漂移".into());
            }
            crate::recovery::inputs::domain::validate_plan(&inputs.plan, &inputs.root)
        },
    )
}

fn run_with(
    plan: &Value,
    root: &Path,
    directory: &Path,
    transport: &impl Transport,
    check: impl Fn() -> Result<()>,
) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(900);
    check()?;
    let (path, samples) = input::create(plan, root, directory)?;
    let bound = match client::Bound::capture(plan, root, directory, &path) {
        Ok(bound) => bound,
        Err(error) => {
            // 尚未调用MySQL；仅在仍合法的受管目录独占记录安全失败。
            if crate::local_test_path::validate_local_test_path(
                directory,
                root,
                crate::local_test_path::LocalTestPathKind::ExistingDirectory,
            )
            .is_ok()
            {
                let _ = documents::write_new(
                    &directory.join(format!(
                        "post-batch-failure-{}.json",
                        uuid::Uuid::new_v4().simple()
                    )),
                    &json!({"format_version":1,"kind":"dataset-post-batch-failure","stage":"input-binding",
                        "batch":0,"completed_batches":0,"possible_write":false,"automatic_replay":false}),
                );
            }
            return Err(error);
        }
    };
    let mut client = client::Client {
        bound,
        transport,
        check: &check,
        deadline,
        completed: 0,
        possible_write: false,
    };
    let result = (|| {
        let rows = input::read(plan, &path)?;
        client.bound.verify()?;
        check()?;
        execute(plan, &rows, &mut client)
    })();
    let result = match result {
        Ok(value) => value,
        Err(error) => {
            let receipt = match client.record(
                "batch-rejected",
                json!({"reason":"核对完整前后像和失败记录；禁止自动重放"}),
            ) {
                Ok(path) => format!("失败收据={}", path.display()),
                Err(_) => "不能确认失败收据发布；保留输入与原现场".into(),
            };
            return Err(format!("{error}；阶段=batch-rejected；已完成批数={}；possible_write={}；unknown={}；核对完整前后像及绑定输入，禁止重放；{receipt}",client.completed,client.possible_write,client.possible_write).into());
        }
    };
    Ok(json!({"result":result,"samples":resolve_samples(&samples,&result)?}))
}

fn execute(
    plan: &Value,
    rows: &[Value],
    client: &mut client::Client<'_, impl Transport>,
) -> Result<Value> {
    let maximum = client.query(
        b"SELECT COALESCE(MAX(`id`), 0) FROM `sys_post`;".to_vec(),
        false,
    )?;
    let maximum = std::str::from_utf8(&maximum)
        .map_err(|_| "岗位最大ID不是UTF-8")?
        .trim();
    if maximum.is_empty() || !maximum.bytes().all(|v| v.is_ascii_digit()) {
        return Err("岗位批次无法取得唯一现有最大ID".into());
    }
    let first = maximum
        .parse::<i64>()
        .ok()
        .and_then(|v| v.checked_add(1))
        .filter(|v| *v > 0)
        .ok_or("岗位ID范围超过有符号64位")?;
    let last = first
        .checked_add(rows.len() as i64 - 1)
        .ok_or("岗位ID范围超过有符号64位")?;
    let batch = plan["dataset"]["post_batch_rows"]
        .as_u64()
        .filter(|v| *v > 0)
        .ok_or("岗位批次大小缺失")?;
    for (index, values) in rows.chunks(usize::try_from(batch)?).enumerate() {
        let output = client.query(
            sql::statement(values, first + (index as u64 * batch) as i64).into_bytes(),
            true,
        )?;
        if !std::str::from_utf8(&output)
            .map_err(|_| "岗位输出不是UTF-8")?
            .trim()
            .is_empty()
        {
            return Err("岗位批次写入产生未登记标准输出；可能已写入，禁止重放".into());
        }
        client.completed += 1;
    }
    Ok(
        json!({"format_version":1,"kind":"restore-reference-post-batch","plan_sha256":model::hash(plan)?,
        "input_sha256":client.bound.input_sha(),"rows":rows.len(),"batch_rows":batch,
        "batches":client.completed,"first_id":first.to_string(),"last_id":last.to_string()}),
    )
}
pub(super) fn row(plan: &Value, index: usize, row: u64) -> Value {
    json!({"tenant_id":model::tenant(plan,index),"index":row,"code":format!("{}-{row}",plan["id"].as_str().unwrap_or("")),"name":format!("恢复样本{row}"),"sort":row%1000})
}
pub(super) fn resolve_samples(samples: &[Value], result: &Value) -> Result<Vec<Value>> {
    let first = model::positive_id(&result["first_id"])?.parse::<i64>()?;
    let last = model::positive_id(&result["last_id"])?.parse::<i64>()?;
    let rows = model::integer(&result["rows"], 1, 1_000_000, "批次行数")?;
    if result["kind"] != "restore-reference-post-batch"
        || first.checked_add(rows as i64 - 1) != Some(last)
    {
        return Err("岗位批次结果不完整".into());
    }
    samples.iter().map(|sample| {
        let position=sample["position"].as_u64().filter(|v|*v<rows).ok_or("样本位置越界")?;
        Ok(json!({"identityIndex":sample["identityIndex"],"id":(first+position as i64).to_string(),"code":sample["row"]["code"],"name":sample["row"]["name"]}))
    }).collect()
}
pub(super) fn scrub(mut command: std::process::Command) -> std::process::Command {
    command.env_clear();
    for (name, value) in std::env::vars_os() {
        if crate::devex::runtime_native::entry::allowed_environment(&name.to_string_lossy()) {
            command.env(name, value);
        }
    }
    command
}

#[cfg(test)]
#[path = "../../../tests/unit/recovery/dataset_prepare/batch.rs"]
mod tests;

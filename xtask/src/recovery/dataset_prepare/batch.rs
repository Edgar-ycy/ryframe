use super::{
    documents,
    model::{self, Inputs},
};
use crate::{Result, check::consumer_context::python};
use serde_json::{Value, json};
use std::{fs::OpenOptions, io::Write, path::Path, time::Duration};

pub(super) fn run(inputs: &Inputs, directory: &Path) -> Result<Value> {
    let settings = &inputs.plan["dataset"];
    let records = settings["records"].as_u64().ok_or("记录数缺失")?;
    let api = settings["api_validation_posts"]
        .as_u64()
        .ok_or("API记录数缺失")?;
    let path = directory.join("post-batch.ndjson");
    let mut stream = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let mut samples = Vec::new();
    let mut position = 0u64;
    for index in 0..11 {
        let count = model::count(records, index);
        for row_index in api..count {
            let row = row(&inputs.plan, index, row_index);
            if row_index == api || row_index == count - 1 {
                samples.push(json!({"identityIndex":index,"position":position,"row":row}));
            }
            serde_json::to_writer(&mut stream, &row)?;
            stream.write_all(b"\n")?;
            position += 1;
        }
    }
    stream.sync_all()?;
    drop(stream);
    if position != records - api * 11 {
        return Err("岗位批次行数与计划规模不一致".into());
    }
    let program = python::selected_program(&inputs.root)?;
    let command = python::command(
        &inputs.root,
        &program,
        &[
            "-X",
            "utf8",
            "-B",
            "tools/python/restore_reference_post_batch.py",
            "--backend-dir",
            inputs.root.to_str().ok_or("后端路径不是UTF-8")?,
            "--plan",
            inputs.plan_path.to_str().ok_or("计划路径不是UTF-8")?,
            "--input",
            path.to_str().ok_or("批次路径不是UTF-8")?,
        ],
    );
    let output = python::output(&inputs.root, scrub(command), None, Duration::from_secs(900))
        .map_err(|_| "岗位批次准备失败；含未知写入，禁止重放")?;
    if !output.status.success() {
        return Err("岗位批次准备失败；保留输入和阶段收据，含未知写入，禁止重放".into());
    }
    let result = documents::parse_fragment(
        std::str::from_utf8(&output.stdout).map_err(|_| "岗位批次结果不是UTF-8")?,
        "岗位批次结果",
    )?;
    let batch = settings["post_batch_rows"]
        .as_u64()
        .ok_or("岗位批次大小缺失")?;
    if result["plan_sha256"] != model::hash(&inputs.plan)?
        || result["rows"] != position
        || result["batch_rows"] != batch
        || result["batches"] != position.div_ceil(batch)
        || result["input_sha256"] != model::sha(&std::fs::read(&path)?)
    {
        return Err("岗位批次结果未绑定计划、输入或精确行数".into());
    }
    Ok(json!({"result":result,"samples":resolve_samples(&samples,&result)?}))
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

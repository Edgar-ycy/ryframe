use super::{documents, model, row};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub(super) fn create(plan: &Value, root: &Path, directory: &Path) -> Result<(PathBuf, Vec<Value>)> {
    validate_local_test_path(directory, root, LocalTestPathKind::ExistingDirectory)?;
    let expected = model::path(&plan["work_dir"])?.join("dataset");
    if fs::canonicalize(directory)? != fs::canonicalize(expected)? {
        return Err("岗位批次必须位于当前work/dataset".into());
    }
    let path = directory.join("post-batch.ndjson");
    validate_local_test_path(&path, root, LocalTestPathKind::OutputFile)?;
    let mut stream = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let records = plan["dataset"]["records"].as_u64().ok_or("岗位总数缺失")?;
    let api = plan["dataset"]["api_validation_posts"]
        .as_u64()
        .ok_or("API数缺失")?;
    let mut samples = Vec::new();
    let mut position = 0;
    for index in 0..11 {
        let count = model::count(records, index);
        for row_index in api..count {
            let value = row(plan, index, row_index);
            if row_index == api || row_index == count - 1 {
                samples.push(json!({"identityIndex":index,"position":position,"row":value}));
            }
            serde_json::to_writer(&mut stream, &value)?;
            stream.write_all(b"\n")?;
            position += 1;
        }
    }
    stream.sync_all()?;
    Ok((path, samples))
}

pub(super) fn read(plan: &Value, path: &Path) -> Result<Vec<Value>> {
    let records = plan["dataset"]["records"].as_u64().ok_or("岗位总数缺失")?;
    let api = plan["dataset"]["api_validation_posts"]
        .as_u64()
        .ok_or("API数缺失")?;
    let expected = records
        .checked_sub(api.checked_mul(11).ok_or("API数溢出")?)
        .ok_or("岗位数无效")?;
    let mut next = [api; 11];
    let mut rows = Vec::new();
    for line in fs::read_to_string(path)?.lines() {
        let value = documents::parse_fragment(line, "岗位批次行")
            .map_err(|_| "岗位批次行不是有效JSON或包含重复字段")?;
        let index = (0..11)
            .find(|i| value["tenant_id"] == model::tenant(plan, *i))
            .ok_or("岗位租户越界")?;
        if next[index] >= model::count(records, index) || value != row(plan, index, next[index]) {
            return Err("岗位批次字段、完整索引或确定性内容不符".into());
        }
        next[index] += 1;
        rows.push(value);
    }
    if rows.len() as u64 != expected || (0..11).any(|i| next[i] != model::count(records, i)) {
        return Err("岗位批次缺少完整租户行或精确数量".into());
    }
    Ok(rows)
}

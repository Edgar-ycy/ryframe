use super::super::{file_binding, portable_path, read_json, review};
use super::{guards, protocol::check};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{digest, json as native_json},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const LIMIT: u64 = 4 * 1024 * 1024;
const KEYS: &[(&str, &str)] = &[
    ("dedicated-a", "dedicated"),
    ("dedicated-b", "dedicated"),
    ("shared", "shared"),
    ("shared-control", "shared"),
];

fn backend(value: &Value) -> Result<PathBuf> {
    let path = Path::new(value.as_str().ok_or("后端路径缺失")?);
    check(path.is_absolute(), "后端路径必须是绝对路径")?;
    Ok(portable_path(&std::fs::canonicalize(path)?))
}
fn document(root: &Path, value: &Value) -> Result<(PathBuf, Value)> {
    let path = Path::new(value.as_str().ok_or("夹具输入路径缺失")?);
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    };
    validate_local_test_path(&path, root, LocalTestPathKind::ExistingFile)?;
    let path = portable_path(&std::fs::canonicalize(path)?);
    let value = read_json(&path, "夹具输入", LIMIT)?;
    Ok((path, value))
}

pub(super) fn create(args: &Value) -> Result<Value> {
    native_json::exact(
        args,
        &["backend", "review", "fixture", "maintenance_build", "side"],
        "夹具环境计划请求",
    )?;
    let root = backend(&args["backend"])?;
    let (review_file, review_value) = document(&root, &args["review"])?;
    let (fixture_file, fixture) = document(&root, &args["fixture"])?;
    let (maintenance_file, maintenance) = document(&root, &args["maintenance_build"])?;
    review::validate_review(&review_value)?;
    guards::fixture(&fixture)?;
    guards::maintenance(&maintenance)?;
    check(
        ["source", "protected_target"].iter().all(|side| {
            review_value["reference"][*side]["databases"]
                .as_array()
                .is_some_and(Vec::is_empty)
        }),
        "新的隔离参考夹具不得读取或复用历史来源数据库",
    )?;
    let side = args["side"].as_str().ok_or("侧缺失")?;
    let selected = &review_value["scopes"][side];
    check(selected.is_object(), "夹具环境必须选择已审阅侧")?;
    let databases = selected["databases"].as_array().ok_or("数据库列表缺失")?;
    let keys = databases
        .iter()
        .filter_map(|item| item["key"].as_str())
        .collect::<BTreeSet<_>>();
    check(
        databases.len() == 4 && keys == KEYS.iter().map(|(key, _)| *key).collect(),
        "夹具侧必须精确声明四个数据库",
    )?;
    let mut names = Vec::new();
    for scope in review_value["scopes"]
        .as_object()
        .ok_or("三侧缺失")?
        .values()
    {
        for item in scope["databases"].as_array().ok_or("数据库列表缺失")? {
            names.push(
                item["database"]
                    .as_str()
                    .ok_or("数据库名缺失")?
                    .to_lowercase(),
            );
        }
    }
    check(
        names.len() == 12 && names.iter().collect::<BTreeSet<_>>().len() == 12,
        "三侧夹具数据库名称不得重叠",
    )?;
    let binding = file_binding(&review_file, "审阅收据")?;
    let mut result = json!({
        "format_version":1, "kind":"reference-fixture-environment-plan",
        "review":binding, "fixture":file_binding(&fixture_file,"业务 crate 收据")?,
        "maintenance_build":file_binding(&maintenance_file,"维护构建收据")?,
        "side":side, "scope_id":selected["scope_id"],
        "databases":KEYS.iter().map(|(key, mode)| {
            let item = databases.iter().find(|item| item["key"] == *key).expect("已检查四库");
            json!({"key":key,"database":item["database"],"mode":mode})
        }).collect::<Vec<_>>(),
        "object_endpoint":selected["objects"]["endpoint"], "redis_url":selected["redis"]["url"],
        "historical_data_used":false,"remote_writes":0,
    });
    result["review"]["canonical_sha256"] = json!(digest::plan_hash(&review_value)?);
    for (path, original, role) in [
        (&review_file, &review_value, "review"),
        (&fixture_file, &fixture, "fixture"),
        (&maintenance_file, &maintenance, "maintenance_build"),
    ] {
        let current = read_json(path, "夹具输入", LIMIT)?;
        let actual = file_binding(path, "夹具输入")?;
        check(
            current == *original
                && ["path", "bytes", "sha256"]
                    .iter()
                    .all(|key| result[role][*key] == actual[*key]),
            "夹具环境输入在计划期间发生变化",
        )?;
    }
    result["sha256"] = json!(digest::plan_hash(&result)?);
    Ok(result)
}

pub(super) fn inputs(args: &Value) -> Result<Value> {
    native_json::exact(args, &["backend", "plan"], "夹具环境输入复核")?;
    let root = backend(&args["backend"])?;
    let plan = &args["plan"];
    let mut result = Vec::new();
    for role in ["review", "fixture", "maintenance_build"] {
        let descriptor = &plan[role];
        let fields: &[&str] = if role == "review" {
            &["path", "bytes", "sha256", "canonical_sha256"]
        } else {
            &["path", "bytes", "sha256"]
        };
        native_json::exact(descriptor, fields, "夹具计划输入")?;
        let (path, value) = document(&root, &descriptor["path"])?;
        let actual = file_binding(&path, "夹具计划输入")?;
        let valid = ["path", "bytes", "sha256"]
            .iter()
            .all(|key| descriptor[*key] == actual[*key])
            && (role != "review" || descriptor["canonical_sha256"] == digest::plan_hash(&value)?);
        let message = match role {
            "review" => "夹具环境计划中的审阅收据已变化",
            "fixture" => "夹具环境计划中的业务 crate 收据已变化",
            _ => "夹具环境计划中的维护构建收据已变化",
        };
        check(valid, message)?;
        match role {
            "review" => {
                guards::preflight_binding(&value)?;
            }
            "fixture" => guards::fixture(&value)?,
            _ => guards::maintenance(&value)?,
        }
        result.push(json!(path));
        result.push(value);
    }
    Ok(json!({"inputs":result}))
}

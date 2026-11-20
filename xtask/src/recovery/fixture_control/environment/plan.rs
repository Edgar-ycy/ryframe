use super::super::{portable_path, review};
use super::{
    guards,
    protocol::{GuardError, check},
};
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

#[path = "plan/checked.rs"]
mod checked;
pub(in crate::recovery::fixture_control) use checked::CheckedPlan;
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
fn input_path(root: &Path, value: &Value) -> Result<PathBuf> {
    let path = Path::new(value.as_str().ok_or("夹具输入路径缺失")?);
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    };
    validate_local_test_path(&path, root, LocalTestPathKind::ExistingFile)?;
    Ok(portable_path(&std::fs::canonicalize(path)?))
}
pub(super) fn create(args: &Value) -> Result<Value> {
    native_json::exact(
        args,
        &[
            "backend",
            "review",
            "execution_source",
            "maintenance_build",
            "side",
        ],
        "夹具环境计划请求",
    )?;
    let root = backend(&args["backend"])?;
    let inputs = checked::Inputs::load(&root, args)?;
    let result = inputs.calculate();
    let after = inputs.verify(result.as_ref().ok());
    match (result, after) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(after)) => {
            // 后验重算可能重复同一条已脱敏的拒绝，保留原错误类型供协议返回。
            if let (Some(first), Some(repeated)) = (
                error.downcast_ref::<GuardError>(),
                after.downcast_ref::<GuardError>(),
            ) && first.0 == repeated.0
            {
                return Err(error);
            }
            Err(format!("{error}；输入后验同时失败：{after}").into())
        }
    }
}

fn calculate(
    review_value: &Value,
    publication: &Value,
    maintenance: &Value,
    review_binding: &Value,
    publication_binding: &Value,
    maintenance_binding: &Value,
    side: &str,
) -> Result<Value> {
    review::validate_review(review_value)?;
    guards::maintenance(maintenance)?;
    let selected = selected(review_value, side)?;
    let original = &publication["original"]["source_pair"]["sources"];
    let execution_root = &publication["published"]["backend"]["path"];
    check(
        review_value["execution_source"] == *publication_binding
            && review_value["sources"]["backend_head"] == original["backend"]["head"]
            && review_value["sources"]["frontend_head"] == original["frontend"]["head"]
            && selected["backend_dir"] == *execution_root,
        "审阅计划与执行来源或原始双仓来源不一致",
    )?;
    let databases = selected["databases"].as_array().ok_or("数据库列表缺失")?;
    let mut result = json!({
        "format_version":1, "kind":"reference-fixture-environment-plan",
        "review":review_binding,
        "execution_source":publication_binding,
        "fixture":publication["original"]["fixture_receipt"],
        "execution_backend":execution_root,
        "maintenance_build":maintenance_binding,
        "side":side, "scope_id":selected["scope_id"],
        "databases":KEYS.iter().map(|(key, mode)| {
            let item = databases.iter().find(|item| item["key"] == *key).expect("已检查四库");
            json!({"key":key,"database":item["database"],"mode":mode})
        }).collect::<Vec<_>>(),
        "object_endpoint":selected["objects"]["endpoint"], "redis_url":selected["redis"]["url"],
        "historical_data_used":false,"remote_writes":0,
    });
    result["review"]["canonical_sha256"] = json!(digest::plan_hash(review_value)?);
    result["sha256"] = json!(digest::plan_hash(&result)?);
    Ok(result)
}

fn selected(review: &Value, side: &str) -> Result<Value> {
    check(
        ["source", "protected_target"].iter().all(|side| {
            review["reference"][*side]["databases"]
                .as_array()
                .is_some_and(Vec::is_empty)
        }),
        "新的隔离参考夹具不得读取或复用历史来源数据库",
    )?;
    let selected = &review["scopes"][side];
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
    for scope in review["scopes"].as_object().ok_or("三侧缺失")?.values() {
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
    Ok(selected.clone())
}

pub(in crate::recovery) fn inputs(args: &Value) -> Result<Value> {
    native_json::exact(args, &["backend", "plan"], "夹具环境输入复核")?;
    let root = backend(&args["backend"])?;
    let checked = CheckedPlan::load(&root, &args["plan"])?;
    let value = checked.projection();
    checked.verify()?;
    Ok(value)
}

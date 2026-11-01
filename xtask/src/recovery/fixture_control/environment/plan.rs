use super::super::{file_binding, portable_path, read_json, review, source_pair::execution};
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
fn document(root: &Path, value: &Value) -> Result<(PathBuf, Value)> {
    let path = input_path(root, value)?;
    let value = read_json(&path, "夹具输入", LIMIT)?;
    Ok((path, value))
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
    let (review_file, review_value) = document(&root, &args["review"])?;
    let publication_file = input_path(&root, &args["execution_source"])?;
    let publication_binding = file_binding(&publication_file, "执行来源")?;
    let publication = execution::verify(&root, &publication_binding)?;
    let (maintenance_file, maintenance) = document(&root, &args["maintenance_build"])?;
    review::validate_review(&review_value)?;
    guards::maintenance(&maintenance)?;
    let side = args["side"].as_str().ok_or("侧缺失")?;
    let selected = selected(&review_value, side)?;
    let original = &publication["original"]["source_pair"]["sources"];
    let execution_root = &publication["published"]["backend"]["path"];
    check(
        review_value["execution_source"] == publication_binding
            && review_value["sources"]["backend_head"] == original["backend"]["head"]
            && review_value["sources"]["frontend_head"] == original["frontend"]["head"]
            && selected["backend_dir"] == *execution_root,
        "审阅计划与执行来源或原始双仓来源不一致",
    )?;
    let databases = selected["databases"].as_array().ok_or("数据库列表缺失")?;
    let mut result = json!({
        "format_version":1, "kind":"reference-fixture-environment-plan",
        "review":file_binding(&review_file, "审阅收据")?,
        "execution_source":publication_binding,
        "fixture":publication["original"]["fixture_receipt"],
        "execution_backend":execution_root,
        "maintenance_build":file_binding(&maintenance_file, "维护构建收据")?,
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
    check(
        execution::verify(&root, &publication_binding)? == publication,
        "夹具计划期间执行来源变化",
    )?;
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
    let plan = &args["plan"];
    let expected = create(&json!({"backend":root,"review":plan["review"]["path"],
        "execution_source":plan["execution_source"]["path"],
        "maintenance_build":plan["maintenance_build"]["path"],"side":plan["side"]}))?;
    check(expected == *plan, "夹具环境计划或完整输入绑定已变化")?;
    let (review_path, review) = document(&root, &plan["review"]["path"])?;
    guards::preflight_binding(&review)?;
    let publication_path = input_path(&root, &plan["execution_source"]["path"])?;
    let publication = execution::verify(&root, &plan["execution_source"])?;
    let (maintenance_path, maintenance) = document(&root, &plan["maintenance_build"]["path"])?;
    guards::maintenance(&maintenance)?;
    let terminal = create(&json!({"backend":root,"review":review_path,
        "execution_source":publication_path,"maintenance_build":maintenance_path,"side":plan["side"]}))?;
    check(terminal == expected, "夹具环境输入复核期间变化")?;
    Ok(
        json!({"inputs":[review_path,review,publication_path,publication,maintenance_path,maintenance]}),
    )
}

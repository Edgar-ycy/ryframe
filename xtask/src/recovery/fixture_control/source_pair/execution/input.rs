use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::{digest, json::exact, source},
};

use super::super::super::{file_binding, git_root, portable_path, read_json};
use super::super::{MAX_RECEIPT_BYTES, validate_clean_snapshot, validate_fixture_root};

pub(super) struct Side {
    pub root: PathBuf,
    pub inventory: source::Inventory,
    pub patch: Vec<u8>,
    pub asset: Value,
    pub files: Vec<source::FileRecord>,
}

pub(super) struct Input {
    pub receipt: Value,
    pub sides: Vec<(&'static str, Side)>,
}

pub(super) fn load(backend: &Path) -> Result<Input> {
    validate_absolute_path(backend, LocalTestPathKind::ExistingDirectory)?;
    let backend = portable_path(&fs::canonicalize(backend)?);
    let parent = backend.parent().ok_or("业务后端缺少 fixture 根")?;
    let path = parent.join("fixture.json");
    validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
    let binding = file_binding(&path, "业务 fixture")?;
    let fixture = read_json(&path, "业务 fixture", MAX_RECEIPT_BYTES)?;
    exact(
        &fixture,
        &[
            "format_version",
            "fixture",
            "status",
            "fixture_sha256",
            "sources",
            "paths",
            "generated",
        ],
        "业务 fixture",
    )?;
    if fixture["format_version"] != 1
        || fixture["fixture"] != "business"
        || fixture["status"] != "ready"
    {
        return Err("执行来源要求已 ready 的正式业务 fixture".into());
    }
    for field in ["sources", "paths", "generated"] {
        exact(
            &fixture[field],
            &["backend", "frontend"],
            "fixture 双仓字段",
        )?;
    }
    let (mut sides, current) = capture_sides(&fixture, parent, &backend)?;
    let frontend = &mut sides
        .iter_mut()
        .find(|(name, _)| *name == "frontend")
        .ok_or("业务 fixture 缺少前端来源")?
        .1;
    let contract_assets = super::assets::load(&backend, &frontend.root)?;
    frontend.files = super::assets::expected(&frontend.inventory.files, &contract_assets)?;
    frontend.asset = contract_assets.clone();
    if digest::file(&backend.join("crates/order-business/src/resources/mod.rs"))?.sha256
        != fixture["fixture_sha256"]
            .as_str()
            .ok_or("fixture 模型摘要缺失")?
    {
        return Err("业务模型与正式 fixture 模型摘要不一致".into());
    }
    if file_binding(&path, "业务 fixture")? != binding
        || read_json(&path, "业务 fixture", MAX_RECEIPT_BYTES)? != fixture
        || super::assets::load(&backend, &frontend.root)? != contract_assets
    {
        return Err("读取期间业务 fixture 收据变化".into());
    }
    Ok(Input {
        receipt: json!({
            "fixture_receipt": binding,
            "source_pair": {"format_version":1,"kind":"reference-fixture-source-pair", "fixture_receipt":binding,"sources":fixture["sources"]},
            "paths":fixture["paths"], "generated":fixture["generated"], "current":current,
            "contract_assets":contract_assets,
        }),
        sides,
    })
}

fn capture_sides(
    fixture: &Value,
    parent: &Path,
    backend: &Path,
) -> Result<(Vec<(&'static str, Side)>, Value)> {
    let mut sides = Vec::new();
    let mut current = serde_json::Map::new();
    for name in ["backend", "frontend"] {
        let root = PathBuf::from(
            fixture["paths"][name]
                .as_str()
                .ok_or("fixture 源码路径无效")?,
        );
        validate_absolute_path(&root, LocalTestPathKind::ExistingDirectory)?;
        if name == "backend" {
            validate_fixture_root(&root, parent, backend)?;
        } else if root.parent() != Some(parent)
            || git_root(&root, "前端源码")? != root
            || root.as_path() == backend
        {
            return Err("fixture 前端必须是独立的同级 Git 根".into());
        }
        validate_clean_snapshot(&fixture["sources"][name], "fixture 原始来源")?;
        let side = capture(&root, &fixture["generated"][name])?;
        if fixture["sources"][name]["head"] != fixture["generated"][name]["head"] {
            return Err("生成源码不是原 fixture 的同一 clean HEAD".into());
        }
        current.insert(name.into(), source::inventory_value(&side.inventory));
        sides.push((name, side));
    }
    Ok((sides, Value::Object(current)))
}

fn capture(root: &Path, generated: &Value) -> Result<Side> {
    let inventory = source::capture(root)?;
    let snapshot = &inventory.source.snapshot;
    let expected = json!({"head":snapshot.head,"patch_sha256":snapshot.patch_sha256,
        "files":snapshot.files.iter().map(|v| json!({"path":v.path,"sha256":v.sha256})).collect::<Vec<_>>()});
    if generated != &expected {
        return Err("当前生成源码不等于 fixture 冻结的完整差异和未跟踪清单".into());
    }
    require_source_changes(root, &inventory)?;
    let patch = super::git::patch(root)?;
    if crate::devex::runtime_native::artifacts::sha(&patch) != snapshot.patch_sha256
        || source::capture(root)? != inventory
    {
        return Err("读取生成补丁期间源码漂移".into());
    }
    Ok(Side {
        root: root.to_path_buf(),
        files: inventory.files.clone(),
        inventory,
        patch,
        asset: Value::Null,
    })
}

fn require_source_changes(root: &Path, inventory: &source::Inventory) -> Result<()> {
    // clean HEAD 中受管的配置模板属于源码；生成阶段不得把运行配置或产物加入发布。
    let changed = super::git::run(root, &["diff", "--name-only", "-z", "HEAD", "--", "."])?;
    let tracked = changed
        .split(|v| *v == 0)
        .filter(|v| !v.is_empty())
        .map(std::str::from_utf8)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for name in tracked.into_iter().chain(
        inventory
            .source
            .snapshot
            .files
            .iter()
            .map(|v| v.path.as_str()),
    ) {
        if ["config", ".local-tests", "target", "dist", "node_modules"]
            .iter()
            .any(|prefix| {
                name == *prefix
                    || name
                        .strip_prefix(*prefix)
                        .is_some_and(|v| v.starts_with('/'))
            })
        {
            return Err("冻结生成差异含运行配置或构建产物；保留原 fixture，配置应独立绑定".into());
        }
    }
    Ok(())
}

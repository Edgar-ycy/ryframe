use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::{digest, json::exact, source, target_verify::guard},
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

/// 原 fixture 首读前保护文件；单次观察不构成完整来源接受结果。
pub(super) struct Fixture {
    backend: PathBuf,
    path: PathBuf,
    binding: Value,
    value: Value,
    protection: guard::Inputs,
}

pub(super) fn load(backend: &Path) -> Result<Input> {
    let fixture = Fixture::open(backend)?;
    let before = fixture.observe();
    let after = fixture.observe();
    let files = fixture.verify();
    match before {
        Ok(before) => {
            let after = after.and_then(|after| {
                super::window::check(
                    before.receipt == after.receipt,
                    "读取期间原 fixture 或完整生成来源变化",
                )
            });
            super::window::finish([after, files])?;
            Ok(before)
        }
        Err(error) => {
            let after = super::window::finish([after.map(|_| ()), files]);
            Err(super::window::failure(error, after))
        }
    }
}

impl Fixture {
    pub(super) fn open(backend: &Path) -> Result<Self> {
        validate_absolute_path(backend, LocalTestPathKind::ExistingDirectory)?;
        let backend = portable_path(&fs::canonicalize(backend)?);
        let path = backend
            .parent()
            .ok_or("业务后端缺少 fixture 根")?
            .join("fixture.json");
        validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
        let protection = guard::Inputs::open([path.clone()])?;
        let result: Result<(Value, Value)> = (|| {
            let binding = file_binding(&path, "业务 fixture")?;
            let value = read_json(&path, "业务 fixture", MAX_RECEIPT_BYTES)?;
            validate(&value)?;
            Ok((binding, value))
        })();
        let after = protection.check();
        let (binding, value) = match result {
            Ok(value) => {
                after?;
                value
            }
            Err(error) => return Err(super::window::failure(error, after)),
        };
        Ok(Self {
            backend,
            path,
            binding,
            value,
            protection,
        })
    }

    /// 仅 execution 窗口及本模块双端包装使用，不向其他职责暴露单次核验。
    pub(super) fn observe(&self) -> Result<Input> {
        let backend = self.side("backend");
        let frontend = self.side("frontend");
        let assets = self.assets();
        let mut sides = Vec::new();
        let mut checks = Vec::new();
        for (name, result) in [("backend", backend), ("frontend", frontend)] {
            match result {
                Ok(side) => sides.push((name, side)),
                Err(error) => checks.push(Err(format!("生成来源 {name}：{error}").into())),
            }
        }
        let assets = match assets {
            Ok(value) => Some(value),
            Err(error) => {
                checks.push(Err(error));
                None
            }
        };
        super::window::finish(checks)?;
        self.assemble(sides, assets.ok_or("生成来源缺少已核验契约资产")?)
    }

    pub(super) fn verify(&self) -> Result<()> {
        let binding = file_binding(&self.path, "业务 fixture").and_then(|value| {
            super::window::check(value == self.binding, "原 fixture 文件绑定在读取期间变化")
        });
        let document = read_json(&self.path, "业务 fixture", MAX_RECEIPT_BYTES).and_then(|value| {
            super::window::check(value == self.value, "原 fixture JSON 在读取期间变化")
        });
        super::window::finish([self.protection.check(), binding, document])
    }

    fn side(&self, name: &'static str) -> Result<Side> {
        let parent = self.backend.parent().ok_or("业务后端缺少 fixture 根")?;
        let root = PathBuf::from(
            self.value["paths"][name]
                .as_str()
                .ok_or("fixture 源码路径无效")?,
        );
        validate_absolute_path(&root, LocalTestPathKind::ExistingDirectory)?;
        if name == "backend" {
            validate_fixture_root(&root, parent, &self.backend)?;
        } else if root.parent() != Some(parent)
            || git_root(&root, "前端源码")? != root
            || root == self.backend
        {
            return Err("fixture 前端必须是独立的同级 Git 根".into());
        }
        validate_clean_snapshot(&self.value["sources"][name], "fixture 原始来源")?;
        let side = capture(&root, &self.value["generated"][name])?;
        if self.value["sources"][name]["head"] != self.value["generated"][name]["head"] {
            return Err("生成源码不是原 fixture 的同一 clean HEAD".into());
        }
        Ok(side)
    }

    fn assets(&self) -> Result<Value> {
        let frontend = Path::new(
            self.value["paths"]["frontend"]
                .as_str()
                .ok_or("前端路径缺失")?,
        );
        let assets = super::assets::load(&self.backend, frontend)?;
        let model = (|| {
            if digest::file(
                &self
                    .backend
                    .join("crates/order-business/src/resources/mod.rs"),
            )?
            .sha256
                != self.value["fixture_sha256"]
                    .as_str()
                    .ok_or("fixture 模型摘要缺失")?
            {
                return Err("业务模型与正式 fixture 模型摘要不一致".into());
            }
            Ok(())
        })();
        let after = super::assets::load(&self.backend, frontend)
            .and_then(|value| super::window::check(value == assets, "读取期间业务契约资产变化"));
        super::window::finish([model, after])?;
        Ok(assets)
    }

    fn assemble(&self, mut sides: Vec<(&'static str, Side)>, assets: Value) -> Result<Input> {
        let mut current = serde_json::Map::new();
        for (name, side) in &sides {
            current.insert((*name).into(), source::inventory_value(&side.inventory));
        }
        let frontend = &mut sides
            .iter_mut()
            .find(|(name, _)| *name == "frontend")
            .ok_or("业务 fixture 缺少前端来源")?
            .1;
        frontend.files = super::assets::expected(&frontend.inventory.files, &assets)?;
        frontend.asset = assets.clone();
        Ok(Input {
            receipt: json!({
                "fixture_receipt": self.binding,
                "source_pair": {"format_version":1,"kind":"reference-fixture-source-pair",
                    "fixture_receipt":self.binding,"sources":self.value["sources"]},
                "paths":self.value["paths"],"generated":self.value["generated"],
                "current":current,"contract_assets":assets,
            }),
            sides,
        })
    }
}

fn validate(fixture: &Value) -> Result<()> {
    exact(
        fixture,
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
    Ok(())
}

fn capture(root: &Path, generated: &Value) -> Result<Side> {
    let inventory = source::capture(root)?;
    let snapshot = &inventory.source.snapshot;
    let expected = json!({"head":snapshot.head,"patch_sha256":snapshot.patch_sha256,
        "files":snapshot.files.iter().map(|v|json!({"path":v.path,"sha256":v.sha256})).collect::<Vec<_>>()});
    if generated != &expected {
        return Err("当前生成源码不等于 fixture 冻结的完整差异和未跟踪清单".into());
    }
    require_source_changes(root, &inventory)?;
    let patch = super::git::patch(root)?;
    if crate::devex::runtime_native::artifacts::sha(&patch) != snapshot.patch_sha256 {
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

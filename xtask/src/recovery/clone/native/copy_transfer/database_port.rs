//! 按原复制计划连接四库前后像、只读证据和受账本授权的单次 SQL。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value, json};

use super::{
    super::{
        copy_ledger::session::Session,
        copy_owner::Owner,
        digest,
        export::transport::Transport,
        input, json as document,
        resources::common,
        target_verify::{database::model, guard::Inputs, input::request::KEYS},
    },
    images::{self, DatabaseSide},
    mysql,
    observe_database::{Baseline, Observed, ObservedPair, Side, Window},
    steps::{Basis, Kind, Observation, Step},
};
use crate::{Result, cli::CloneSide, recovery::source::evidence::reference::copy_step_ids};

pub(in crate::recovery::clone::native) struct Bind<'a> {
    pub(crate) window: &'a Window<'a>,
    pub(crate) baseline: &'a Baseline,
    pub(crate) owner: &'a Owner,
    pub(crate) copy_root: &'a Path,
    pub(crate) export_root: &'a Path,
    pub(crate) export: &'a Value,
    pub(crate) declaration: &'a Value,
    pub(crate) plan: &'a Value,
    pub(crate) generation_sha256: &'a str,
}

pub(in crate::recovery::clone::native) struct DatabasePort<'a, T> {
    spec: Bind<'a>,
    transport: &'a T,
    databases: BTreeMap<&'a str, Database<'a>>,
    source_scope: &'a str,
    target_scope: &'a str,
    root: Value,
    inputs: Inputs,
    initial_source: Inventory,
}

pub(in crate::recovery::clone::native) struct Evidence {
    pub(in crate::recovery::clone::native) pair: ObservedPair,
    seals: [Inventory; 2],
    owner: Value,
}

impl Evidence {
    fn check(&self, backend: &Path, owner: &Value) -> Result<()> {
        if &self.owner != owner {
            return Err("四库观察证据不属于当前复制会话".into());
        }
        for seal in &self.seals {
            seal.check(backend)?;
        }
        Ok(())
    }
}

impl<'a, T: Transport> DatabasePort<'a, T> {
    pub(crate) fn open(spec: Bind<'a>, transport: &'a T) -> Result<Self> {
        let backend = spec.window.target.storage.backend;
        let inputs = bind_inputs(&spec, backend)?;
        let source_scope = scope(
            &spec.window.source,
            &spec.declaration["source"],
            &spec.plan["source_scope"],
        )?;
        let target_scope = scope(
            &spec.window.target,
            &spec.declaration["target"],
            &spec.plan["target_scope"],
        )?;
        if source_scope == target_scope {
            return Err("复制四库端口必须绑定两侧独立 scope".into());
        }
        let databases = databases(&spec)?;
        let initial = spec.baseline.source();
        let initial_path = common::bound_file(backend, &initial.receipt, "来源初始四库库存")?;
        let initial_directory = initial_path.parent().ok_or("来源初始库存缺少目录")?;
        let initial_source = Inventory::seal(
            backend,
            spec.copy_root,
            initial_directory,
            CloneSide::Source,
            source_scope,
            initial,
        )?;
        let root = directory_binding(backend, spec.copy_root)?;
        let port = Self {
            spec,
            transport,
            databases,
            source_scope,
            target_scope,
            root,
            inputs,
            initial_source,
        };
        port.check()?;
        Ok(port)
    }

    pub(crate) fn basis(&self, step: &Step<'_>) -> Result<Basis> {
        self.initial_source.check(self.backend())?;
        let (key, database) = self.selected(step)?;
        let original = input::bound_file(self.spec.export_root, &database.exported["artifact"])?;
        let basis = self.spec.baseline.basis(key, &original.digest.sha256)?;
        let (before, after) = images::database_pair(
            self.spec.generation_sha256,
            database.item,
            database.source.image(self.source_scope),
            database.target.image(self.target_scope),
            basis,
        )?;
        self.check()?;
        Ok(Basis { before, after })
    }

    pub(crate) fn observe(
        &self,
        all: &[Step<'_>],
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<Observation<Evidence>> {
        let keys = all
            .iter()
            .map(|step| self.selected(step).map(|(key, _)| key))
            .collect::<Result<BTreeSet<_>>>()?;
        if all.len() != self.databases.len() || !keys.into_iter().eq(self.databases.keys().copied())
        {
            return Err("数据库观察必须选择完整且唯一的四个计划步骤".into());
        }
        let evidence = self.capture(checkpoint)?;
        let images = self.images(&evidence.pair.target)?;
        checkpoint()?;
        Ok(Observation { images, evidence })
    }

    pub(crate) fn check_observation(
        &self,
        images: &Map<String, Value>,
        evidence: &Evidence,
    ) -> Result<()> {
        self.check()?;
        evidence.check(self.backend(), self.spec.owner.binding())?;
        if &self.images(&evidence.pair.target)? != images {
            return Err("数据库账本投影已不匹配原独立观察".into());
        }
        self.check()
    }

    pub(crate) fn write(
        &self,
        step: &Step<'_>,
        session: &mut Session<'_>,
        expected: &Basis,
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<()> {
        checkpoint()?;
        if &self.basis(step)? != expected {
            return Err("数据库写入前后像不属于同一原计划和完整初像".into());
        }
        let id = self.spec.owner.session_id();
        if session
            .steps()?
            .get(&step.id)
            .and_then(|value| value["session"].as_str())
            != Some(id)
        {
            return Err("MySQL 命令证据必须绑定本步骤的实际写入会话".into());
        }
        let request = mysql::Write {
            copy_root: self.spec.copy_root,
            session_id: id,
            export_root: self.spec.export_root,
            item: step.item,
        };
        session.write_once(
            &step.id,
            &expected.before,
            &expected.after,
            step.artifact,
            |authorize| {
                let check = || {
                    self.check_window(checkpoint)?;
                    authorize()
                };
                mysql::write(
                    &self.spec.window.target.storage,
                    &request,
                    self.transport,
                    &check,
                )
            },
        )
    }

    pub(crate) fn verify_source(&self, checkpoint: &dyn Fn() -> Result<()>) -> Result<()> {
        let (source, _) = self.outputs();
        let check = || self.check_window(checkpoint);
        let observed =
            self.spec
                .window
                .observe_source(self.spec.baseline, &source, self.transport, &check)?;
        self.seal(&source, CloneSide::Source, &observed)?;
        check()
    }

    pub(crate) fn verify_target_complete(&self, checkpoint: &dyn Fn() -> Result<()>) -> Result<()> {
        self.initial_source.check(self.backend())?;
        let evidence = self.capture(checkpoint)?;
        self.spec
            .baseline
            .verify_complete_target(&evidence.pair.target)?;
        evidence.check(self.backend(), self.spec.owner.binding())?;
        self.check_window(checkpoint)
    }

    fn capture(&self, checkpoint: &dyn Fn() -> Result<()>) -> Result<Evidence> {
        let (source_dir, target_dir) = self.outputs();
        let check = || self.check_window(checkpoint);
        let pair = self.spec.window.observe(
            self.spec.baseline,
            &source_dir,
            &target_dir,
            self.transport,
            &check,
        )?;
        let source = self.seal(&source_dir, CloneSide::Source, &pair.source)?;
        let target = self.seal(&target_dir, CloneSide::Target, &pair.target)?;
        check()?;
        Ok(Evidence {
            pair,
            seals: [source, target],
            owner: self.spec.owner.binding().clone(),
        })
    }

    fn seal(&self, path: &Path, side: CloneSide, observed: &Observed) -> Result<Inventory> {
        let scope = match side {
            CloneSide::Source => self.source_scope,
            CloneSide::Target => self.target_scope,
        };
        Inventory::seal(
            self.backend(),
            self.spec.copy_root,
            path,
            side,
            scope,
            observed,
        )
    }

    fn selected(&self, step: &Step<'_>) -> Result<(&'a str, &Database<'a>)> {
        let key = document::text(&step.item["key"], "复制数据库逻辑键")?;
        let (key, database) = self
            .databases
            .get_key_value(key)
            .ok_or("数据库不在原复制计划中")?;
        if !database.matches(step) {
            return Err("数据库步骤编号、完整计划项或 SQL 绑定不同".into());
        }
        Ok((*key, database))
    }

    fn images(&self, observed: &Observed) -> Result<Map<String, Value>> {
        if !observed
            .observations
            .keys()
            .map(String::as_str)
            .eq(self.databases.keys().copied())
        {
            return Err("当前目标四库观察没有精确覆盖计划逻辑键".into());
        }
        self.databases
            .iter()
            .map(|(&key, database)| {
                let image = images::database_observed(
                    self.spec.generation_sha256,
                    database.item,
                    database.target.image(self.target_scope),
                    &observed.observations[key],
                )?;
                Ok((database.id.clone(), image))
            })
            .collect()
    }

    fn outputs(&self) -> (PathBuf, PathBuf) {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let root = self.spec.copy_root;
        (
            root.join(format!("source-inventory-{}", &token[..8])),
            root.join(format!("target-inventory-{}", &token[..8])),
        )
    }

    fn backend(&self) -> &Path {
        self.spec.window.target.storage.backend
    }

    fn check_window(&self, checkpoint: &dyn Fn() -> Result<()>) -> Result<()> {
        checkpoint()?;
        self.check()
    }

    fn check(&self) -> Result<()> {
        self.spec.owner.check()?;
        common::directory_identity(self.backend(), &self.root)?;
        self.inputs.checkpoint()?;
        self.initial_source.checkpoint(self.backend())
    }
}

fn bind_inputs(spec: &Bind<'_>, backend: &Path) -> Result<Inputs> {
    let (owner_backend, plan, generation) = spec.owner.ledger_binding();
    let copy_root = common::local_directory(backend, spec.copy_root, "固定复制目录")?;
    let export_root = common::local_directory(backend, spec.export_root, "固定 SQL 导出根")?;
    if fs::canonicalize(owner_backend)? != fs::canonicalize(backend)?
        || spec.window.source.storage.backend != backend
        || spec.plan["plan_sha256"].as_str() != Some(plan)
        || generation != spec.generation_sha256
        || Path::new(document::text(
            &spec.export["artifact_root"],
            "原 SQL 导出根",
        )?) != export_root
        || spec.declaration["artifact_root"] != spec.export["artifact_root"]
    {
        return Err("四库端口与持有人的原计划、代次或 SQL 导出根不同".into());
    }
    let owner_path = common::bound_file(backend, spec.owner.binding(), "复制持有人")?;
    if owner_path.parent() != Some(copy_root.as_path()) {
        return Err("四库端口工作目录与持有人固定目录不同".into());
    }
    let owner = document::read(&owner_path)?;
    let export = common::bound_file(backend, &owner["source_export"], "原复制导出")?;
    let initial = common::bound_file(backend, &owner["initialized"], "原复制初始化")?;
    if export != export_root.join("export.json") {
        return Err("SQL 导出根不属于持有人固定的原导出文件".into());
    }
    let initial_inventory =
        common::bound_file(backend, spec.baseline.initialized_receipt(), "原初始化库存")?;
    let plan_path = common::common_file(backend, &copy_root.join("plan.json"), "原复制计划")?;
    let input_path = common::common_file(backend, &copy_root.join("input.json"), "原复制声明")?;
    let session = copy_root.join("session.json");
    let inputs = Inputs::open([
        export.clone(),
        initial.clone(),
        plan_path.clone(),
        input_path.clone(),
        initial_inventory.clone(),
    ])?;
    if document::read(&export)? != *spec.export
        || document::read(&initial)? != *spec.window.initialized
        || document::read(&plan_path)?["plan"] != *spec.plan
        || document::read(&input_path)? != *spec.declaration
        || document::read(&initial_inventory)?["observations"]
            != serde_json::to_value(spec.baseline.initialized())?
        || document::read(&session)?["source_observation"] != spec.baseline.source_observation()
    {
        return Err("数据库端口不能混用其他会话的原始复制输入或库存".into());
    }
    inputs.check()?;
    spec.owner.check()?;
    Ok(inputs)
}

struct Database<'a> {
    id: String,
    item: &'a Value,
    exported: &'a Value,
    source: DatabaseDeclaration<'a>,
    target: DatabaseDeclaration<'a>,
}

impl Database<'_> {
    fn matches(&self, step: &Step<'_>) -> bool {
        step.kind == Kind::Database
            && step.id == self.id
            && step.item == self.item
            && self.exported["artifact"]["sha256"].as_str() == Some(step.artifact)
    }
}

struct DatabaseDeclaration<'a> {
    value: &'a Value,
    kept: BTreeSet<String>,
}

impl DatabaseDeclaration<'_> {
    fn image<'a>(&'a self, scope: &'a str) -> DatabaseSide<'a> {
        DatabaseSide {
            scope,
            declaration: self.value,
            preserved_tables: &self.kept,
        }
    }
}

fn databases<'a>(spec: &Bind<'a>) -> Result<BTreeMap<&'a str, Database<'a>>> {
    let planned = index(&spec.plan["databases"])?;
    let exports = index(&spec.export["databases"])?;
    if spec.declaration["databases"] != spec.export["databases"] {
        return Err("复制声明的 SQL 转储向量不是原导出".into());
    }
    let source = index(&spec.declaration["source"]["databases"])?;
    let target = index(&spec.declaration["target"]["databases"])?;
    let registered_source = model::databases(&spec.window.source.storage)?;
    let registered_target = model::databases(&spec.window.target.storage)?;
    let source_root = spec.window.source.storage.execution_backend()?;
    let target_root = spec.window.target.storage.execution_backend()?;
    let ids = copy_step_ids(
        &json!({"databases":planned.keys().map(|key| json!({"key":key})).collect::<Vec<_>>(),
        "objects":[]}),
    )?;
    planned
        .iter()
        .zip(document::array(&ids["databases"], "四库步骤标识")?)
        .map(|((&key, &item), id)| {
            let exported = exports[key];
            if item["artifact"] != exported["artifact"] || item["tables"] != exported["tables"] {
                return Err("数据库计划 SQL 和逐表行数未绑定原导出".into());
            }
            Ok((
                key,
                Database {
                    id: document::text(id, "数据库步骤标识")?.to_owned(),
                    item,
                    exported,
                    source: declaration(source[key], &registered_source[key], &source_root)?,
                    target: declaration(target[key], &registered_target[key], &target_root)?,
                },
            ))
        })
        .collect()
}

fn index(value: &Value) -> Result<BTreeMap<&str, &Value>> {
    let mut index = BTreeMap::new();
    for row in document::array(value, "原复制四库向量")? {
        let key = document::text(&row["key"], "原复制数据库逻辑键")?;
        if !KEYS.iter().any(|(known, _, _)| key == *known) || index.insert(key, row).is_some() {
            return Err("复制四库向量包含重复或未登记的逻辑目标".into());
        }
    }
    if index.len() != KEYS.len() {
        return Err("复制四库向量没有覆盖全部逻辑目标".into());
    }
    Ok(index)
}

fn declaration<'a>(
    value: &'a Value,
    registered: &Value,
    execution: &Path,
) -> Result<DatabaseDeclaration<'a>> {
    if ["key", "kind", "mode", "database", "server_uuid"]
        .iter()
        .any(|field| value[*field] != registered[*field])
    {
        return Err("数据库声明与绑定的本侧物理目标不同".into());
    }
    let (business, all, _) = model::table_sets(execution, value["kind"] == "combined")?;
    Ok(DatabaseDeclaration {
        value,
        kept: all.difference(&business).cloned().collect(),
    })
}

fn scope<'a>(side: &Side<'a>, declaration: &Value, plan: &Value) -> Result<&'a str> {
    let actual = side.storage.target_scope()?;
    if declaration["scope_id"].as_str() != Some(actual) || plan.as_str() != Some(actual) {
        return Err("数据库采集配置、原声明和计划的 scope 不同".into());
    }
    Ok(actual)
}

struct Inventory {
    identity: Value,
    names: BTreeSet<std::ffi::OsString>,
    held: Inputs,
}

impl Inventory {
    fn seal(
        backend: &Path,
        root: &Path,
        directory: &Path,
        side: CloneSide,
        scope: &str,
        observed: &Observed,
    ) -> Result<Self> {
        let identity = directory_binding(backend, directory)?;
        if directory.parent() != Some(root) {
            return Err("四库证据目录必须位于本会话固定根".into());
        }
        let paths = evidence_paths(directory, &observed.evidence_files)?;
        let held = Inputs::open(paths.keys().map(|name| directory.join(name)))?;
        for (name, expected) in &paths {
            let actual = digest::file(&directory.join(name))?;
            if expected["bytes"].as_u64() != Some(actual.bytes)
                || expected["sha256"] != actual.sha256
            {
                return Err("四库观察原始证据在建立保护时变化".into());
            }
        }
        let receipt = common::bound_file(backend, &observed.receipt, "四库观察原库存收据")?;
        let value = document::read(&receipt)?;
        if receipt != directory.join("inventory.json")
            || value["status"] != "side_inventory_captured"
            || value["side"] != side.as_str()
            || value["scope_id"] != scope
            || value["remote_writes"].as_u64() != Some(0)
            || value["observations"] != serde_json::to_value(&observed.observations)?
        {
            return Err("四库原始库存收据与本次独立完整观察不同".into());
        }
        let proof = Self {
            identity,
            names: paths.into_keys().map(Into::into).collect(),
            held,
        };
        proof.check(backend)?;
        Ok(proof)
    }

    fn checkpoint(&self, backend: &Path) -> Result<()> {
        common::directory_identity(backend, &self.identity)?;
        self.held.checkpoint()
    }

    fn check(&self, backend: &Path) -> Result<()> {
        let root = common::directory_identity(backend, &self.identity)?;
        let names = fs::read_dir(root)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<std::io::Result<BTreeSet<_>>>()?;
        if names != self.names {
            return Err("四库观察原始证据缺失或有额外文件".into());
        }
        self.held.check()?;
        self.checkpoint(backend)
    }
}

fn evidence_paths<'a>(
    directory: &Path,
    entries: &'a [Value],
) -> Result<BTreeMap<String, &'a Value>> {
    let mut paths = BTreeMap::new();
    for entry in entries {
        document::exact(
            entry,
            &["path", "type", "bytes", "sha256"],
            "四库原始证据描述",
        )?;
        let name = document::text(&entry["path"], "四库原始证据文件名")?;
        if name.contains('/')
            || name == "failure.json"
            || entry["type"] != "file"
            || paths.insert(name.to_owned(), entry).is_some()
        {
            return Err("四库原始证据不属于独占平面文件集合".into());
        }
        digest::safe_file(directory, name)?;
    }
    let required = ["before", "after"]
        .into_iter()
        .flat_map(|phase| KEYS.map(|(key, _, _)| format!("{phase}-target-{key}.json")))
        .chain(
            [
                "inventory.json",
                "binding-before.json",
                "binding-after.json",
            ]
            .map(str::to_owned),
        );
    if !required.into_iter().all(|name| paths.contains_key(&name)) {
        return Err("四库观察必须保留收据、两轮完整原始输出和配置绑定".into());
    }
    Ok(paths)
}

fn directory_binding(backend: &Path, directory: &Path) -> Result<Value> {
    let path = common::local_directory(backend, directory, "受管库存证据目录")?;
    let (device, inode) = common::device_inode_for(&path, &fs::symlink_metadata(&path)?)?;
    Ok(json!({"path":path,"device":device,"inode":inode}))
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/copy_transfer/database_port.rs"]
mod tests;

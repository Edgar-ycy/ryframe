//! 参考恢复提交清单授权的目标库转储，并分别保留写意图和命令诊断。
use super::{
    Artifact, BTreeSet, CommandOutput, CommandSpec, Invocation, Path, PathBuf, Result,
    StorageInputs, Transport, common, document, domain, fs, invoke, publication, record_exit,
    registered_database,
};
use serde_json::{Value, json};

pub(crate) struct Write<'a> {
    pub(crate) backup_root: &'a Path,
    pub(crate) work: &'a Path,
    pub(crate) task: &'a Value,
    pub(crate) artifact: &'a Value,
    pub(crate) mysql: &'a Value,
}

pub(crate) fn write(
    inputs: &StorageInputs<'_>,
    request: &Write<'_>,
    transport: &impl Transport,
    checkpoint: &dyn Fn() -> Result<()>,
) -> Result<()> {
    checkpoint()?;
    let database = target(inputs, request.task)?;
    let source = artifact(inputs.backend, request)?;
    let (order, allowed) = tables(&request.task["tables"])?;
    let payload = source.ordered_payload(order, &allowed)?;
    let invocation = Invocation::prepare(inputs, &database, Some(request.mysql))?;
    let mut spec = invocation.spec(inputs, payload)?;
    spec.label = "reference-restore-mysql".into();
    source.check()?;
    invocation.check(inputs)?;
    checkpoint()?;
    let evidence = Evidence::publish(inputs.backend, request, &database, &spec)?;
    let (outcome, attempted, unchanged) =
        invoke(&source, &invocation, inputs, spec, transport, &|| {
            evidence.check(inputs.backend)?;
            checkpoint()
        });
    evidence.finish(inputs.backend, &outcome, attempted, unchanged)
}

fn target(inputs: &StorageInputs<'_>, task: &Value) -> Result<Value> {
    document::exact(
        task,
        &["kind", "target", "tables", "relative"],
        "参考恢复数据库任务",
    )?;
    if task["kind"] != "database" {
        return Err("参考恢复 SQL 必须绑定数据库任务".into());
    }
    let database = &task["target"];
    document::exact(
        database,
        &[
            "key",
            "kind",
            "mode",
            "database",
            "server_uuid",
            "defaults_file",
            "defaults_sha256",
        ],
        "参考恢复目标库",
    )?;
    let key = document::text(&database["key"], "参考目标库键")?;
    let name = document::text(&database["database"], "参考目标物理库")?;
    let selected = registered_database(inputs, key, name)?;
    if selected != *database || task["relative"] != format!("databases/{key}.sql") {
        return Err("参考恢复 SQL 的目标物理库或备份文件与已登记任务不同".into());
    }
    Ok(selected)
}

fn artifact<'a>(backend: &'a Path, request: &Write<'_>) -> Result<Artifact<'a>> {
    let value = request.artifact;
    document::exact(
        value,
        &["path", "bytes", "sha256"],
        "参考恢复已绑定 SQL 产物",
    )?;
    let selected = Path::new(document::text(&value["path"], "参考恢复已绑定 SQL 路径")?);
    let relative = document::text(&request.task["relative"], "参考恢复 SQL 相对路径")?;
    Artifact::open_bound(
        backend,
        request.backup_root,
        relative,
        value,
        Some(selected),
    )
}

fn tables(value: &Value) -> Result<(Vec<&str>, BTreeSet<String>)> {
    let mut allowed = BTreeSet::new();
    let values = document::array(value, "参考恢复库存表")?;
    if values.is_empty() {
        return Err("参考恢复 SQL 必须登记全部获准表".into());
    }
    let order = values
        .iter()
        .map(|value| {
            let table = document::text(value, "参考恢复库存表名")?;
            domain::identifier(table)?;
            if domain::EXCLUDED_TABLES.contains(&table) || !allowed.insert(table.to_owned()) {
                return Err("参考恢复 SQL 不得包含保留表或重复表".into());
            }
            Ok(table)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((order, allowed))
}

struct Evidence {
    diagnostic: PathBuf,
    work_id: (u64, u64),
    intent: Value,
    pending: Value,
    value: Value,
}

impl Evidence {
    fn publish(
        backend: &Path,
        request: &Write<'_>,
        database: &Value,
        spec: &CommandSpec,
    ) -> Result<Self> {
        let work = common::local_directory(backend, request.work, "参考恢复 SQL 证据目录")?;
        let work_id = common::device_inode_for(&work, &fs::symlink_metadata(&work)?)?;
        let key = document::text(&database["key"], "参考目标库键")?;
        let token = uuid::Uuid::new_v4().simple();
        let stem = format!("restore-mysql-{key}-{token}");
        let intent_path = work.join(format!("{stem}.intent.json"));
        let diagnostic = work.join(format!("{stem}.diagnostic.json"));
        let intent = json!({
            "format_version":1,"kind":"restore-reference-mysql-intent",
            "target_key":key,"target_database":database["database"],"target_server_uuid":database["server_uuid"],
            "relative":request.task["relative"],"tables":request.task["tables"],"artifact":request.artifact,
            "mysql":request.mysql,"arguments":spec.args,"execution_root":spec.cwd,
            "automatic_retry_permitted":false,"output_policy":"status_only",
        });
        publication::publish(&intent_path, &intent)?;
        let intent_binding = common::binding(&intent_path)?;
        if document::read(&intent_path)? != intent {
            return Err("参考恢复 SQL 写意图在发布期间变化".into());
        }
        let value = json!({
            "format_version":1,"kind":"restore-reference-mysql-diagnostic","intent":intent_binding,
            "returncode":null,"error_type":null,"status":"pending",
            "remote_write_may_have_occurred":true,"automatic_retry_permitted":false,"output_policy":"status_only",
        });
        publication::publish(&diagnostic, &value)?;
        let pending = common::binding(&diagnostic)?;
        if document::read(&diagnostic)? != value {
            return Err("参考恢复 SQL 待定诊断在发布期间变化".into());
        }
        Ok(Self {
            diagnostic,
            work_id,
            intent: intent_binding,
            pending,
            value,
        })
    }

    fn check(&self, backend: &Path) -> Result<()> {
        let work = self
            .diagnostic
            .parent()
            .ok_or("参考恢复 SQL 证据缺少目录")?;
        common::local_directory(backend, work, "参考恢复 SQL 证据目录")?;
        if common::device_inode_for(work, &fs::symlink_metadata(work)?)? != self.work_id
            || common::bound_file(backend, &self.pending, "参考恢复 SQL 待定诊断")?
                != self.diagnostic
        {
            return Err("参考恢复 SQL 证据或目录身份变化，写入结果需要核对".into());
        }
        common::bound_file(backend, &self.intent, "参考恢复 SQL 原写意图")?;
        Ok(())
    }

    fn finish(
        mut self,
        backend: &Path,
        outcome: &Result<CommandOutput>,
        attempted: bool,
        unchanged: bool,
    ) -> Result<()> {
        let passed = record_exit(&mut self.value, outcome, attempted, unchanged);
        self.check(backend)?;
        document::write_replace(&self.diagnostic, &self.value)?;
        common::bound_file(backend, &self.intent, "参考恢复 SQL 原写意图")?;
        if document::read(&self.diagnostic)? != self.value {
            return Err("参考恢复 SQL 诊断发布后变化，写入结果需要核对".into());
        }
        if !passed {
            if !attempted {
                return Err("参考恢复 SQL 启动前证据变化，已保存未启动诊断".into());
            }
            return Err("参考恢复 SQL 写入结果需要核对，已保存诊断，禁止自动重试".into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/copy_transfer/reference_mysql.rs"]
mod tests;

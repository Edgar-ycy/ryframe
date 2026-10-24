//! 来源登记前重新核对完整复制计划、最新外层结果和原账本；不执行复制。
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::clone::native::{
        self, copy_ledger, digest, json as document, lifecycle::state, resources::common,
    },
};

struct Reads<'a> {
    backend: &'a Path,
    files: Vec<(PathBuf, Value)>,
}

impl Reads<'_> {
    fn read(&mut self, path: &Path) -> Result<Value> {
        let binding = common::binding(path)?;
        let value = common::read_bound(self.backend, &binding, "复制完成证据")?;
        self.files.push((path.to_owned(), binding));
        Ok(value)
    }

    fn bound(&mut self, binding: &Value) -> Result<Value> {
        let path = common::bound_file(self.backend, binding, "复制阶段结果")?;
        let value = self.read(&path)?;
        common::bound_file(self.backend, binding, "复制阶段结果")?;
        Ok(value)
    }

    fn check(&self) -> Result<()> {
        for (path, binding) in &self.files {
            common::bound_file(self.backend, binding, "复制完成证据")?;
            if common::binding(path)? != *binding {
                return Err("复制完成证据在核验期间变化；保留现场，禁止自动重放".into());
            }
        }
        Ok(())
    }
}

pub(crate) fn require(backend: &Path, run: &Path, manifest: &Value) -> Result<Value> {
    let output = common::local_directory(
        backend,
        Path::new(document::text(&manifest["copy_directory"], "复制目录")?),
        "复制目录",
    )?;
    let saved = state::load(run, true)?;
    let record = state::attempts(&saved)?
        .iter()
        .rfind(|row| row["stage"] == "copy")
        .ok_or("最新复制阶段尚未通过完整来源和控制器收尾核验")?;
    if record["status"] != "passed" {
        return Err("最新复制阶段失败，不能回退早期成功".into());
    }
    let mut reads = Reads {
        backend,
        files: Vec::new(),
    };
    if reads.read(&run.join("manifest.json"))? != *manifest {
        return Err("复制运行清单不同于当前登记输入".into());
    }
    let session = reads.read(&output.join("session.json"))?;
    let result = reads.read(&output.join("result.json"))?;
    let published = reads.bound(&record["result"])?;
    let exported = export_binding(&mut reads, manifest, &saved)?;
    let plan_path = output.join("plan.json");
    let wrapper = reads.read(&plan_path)?;
    let verified = native::issue(backend, &plan_path, &wrapper)?;
    let plan = &verified["plan"];
    let plan_hash = document::text(&session["plan_sha256"], "复制计划摘要")?;
    let generation = document::text(&session["generation_sha256"], "复制代次摘要")?;
    let ledger = copy_ledger::inspect(backend, &output.join("ledger"), plan_hash, generation)?;
    validate(
        &session,
        &result,
        &published,
        plan,
        &ledger,
        &manifest["initialized"],
        &exported,
    )?;
    if native::issue(backend, &plan_path, &wrapper)? != verified
        || copy_ledger::inspect(backend, &output.join("ledger"), plan_hash, generation)? != ledger
        || state::load(run, true)? != saved
    {
        return Err("复制计划、账本或外层阶段在来源登记核验期间变化".into());
    }
    reads.check()?;
    Ok(plan.clone())
}

fn export_binding(reads: &mut Reads<'_>, manifest: &Value, saved: &Value) -> Result<Value> {
    let descriptor = if !manifest["source_export"].is_null() {
        manifest["source_export"].clone()
    } else {
        let attempt = state::attempts(saved)?
            .iter()
            .rfind(|row| row["stage"] == "export" && row["status"] == "passed")
            .ok_or("复制前必须完成源导出阶段")?;
        reads
            .bound(&attempt["result"])?
            .get("export")
            .cloned()
            .ok_or("源导出结果缺少 export 绑定")?
    };
    common::bound_file(reads.backend, &descriptor, "已完成源导出")?;
    Ok(descriptor)
}

fn validate(
    session: &Value,
    result: &Value,
    published: &Value,
    plan: &Value,
    ledger: &Value,
    initialized: &Value,
    exported: &Value,
) -> Result<()> {
    let mut outer = document::object(published, "复制外层结果")?.clone();
    outer.remove("continuation_receipt");
    if Value::Object(outer) != *result
        || session["initialized"] != *initialized
        || result["status"] != "data_steps_verified"
        || session["source_export"] != *exported
        || result["source_export_sha256"] != session["source_export"]["sha256"]
    {
        return Err("目标启动前须完成本目标的数据复制，且外层结果与内部证据一致".into());
    }
    let databases = document::array(&plan["databases"], "完整复制数据库")?;
    let steps = document::object(&ledger["steps"], "复制账本步骤")?;
    let mut expected = BTreeSet::new();
    for database in databases {
        let key = database.get("key").ok_or("复制数据库缺少 key")?;
        expected.insert(format!(
            "database-{}",
            &digest::plan_hash(&json!({"key":key}))?[..32]
        ));
    }
    let no_writes = ledger["status"] == "partial"
        && steps.is_empty()
        && document::array(&plan["objects"], "复制对象")?.is_empty()
        && result["unchanged_databases"].as_u64() == Some(databases.len() as u64)
        && document::object(&result["unchanged_database_images"], "未写入数据库全像")?
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
            == expected;
    if !(ledger["status"] == "ledger_evidence_complete" || no_writes)
        || result["plan_sha256"] != session["plan_sha256"]
        || result["plan_sha256"] != plan["plan_sha256"]
        || result["generation_sha256"] != session["generation_sha256"]
        || result["written_steps"].as_u64() != Some(steps.len() as u64)
        || result["fresh_target_sha256"] != initialized["sha256"]
    {
        return Err("目标数据复制尚未发布有效的完整账本收据".into());
    }
    if result["pending_target_actions"] != plan["pending_target_actions"] {
        return Err("复制完成证据与当前目标调度处置不同".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/source_registration/copied.rs"]
mod tests;

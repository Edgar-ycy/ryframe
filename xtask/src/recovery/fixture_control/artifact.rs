//! 导出物理证据只读核验；唯一写入是独占创建本地快照收据。
use crate::{
    Result,
    cli::{FixtureArtifactCommand, FixtureArtifactOptions},
    local_test_path::LocalTestPathKind,
};
use serde_json::{Value, json};
use std::path::Path;

#[path = "artifact/database.rs"]
mod database;
#[cfg(test)]
#[path = "../../../tests/unit/recovery/fixture_control/artifact/object_tests.rs"]
mod object_tests;
#[path = "artifact/objects.rs"]
mod objects;
#[path = "artifact/runtime.rs"]
pub(in crate::recovery) mod runtime;
#[path = "artifact/source.rs"]
mod source;
#[cfg(test)]
#[path = "../../../tests/unit/recovery/fixture_control/artifact/tests.rs"]
mod tests;

pub(super) fn run(command: &FixtureArtifactCommand, root: &Path) -> Result<()> {
    let (options, snapshot) = match command {
        FixtureArtifactCommand::Help => return Ok(()),
        FixtureArtifactCommand::Snapshot(options) => (options, true),
        FixtureArtifactCommand::VerifyDeleted(options) => (options, false),
    };
    validate(options, root, snapshot)?;
    let binding = runtime::verify(root, &options.runtime_dir)?;
    let mut database = database::Database::open(&binding.table)?;
    let server = database.identity(&binding.scope)?;
    let objects =
        objects::Objects::new(root, &options.runtime_dir, &binding.table, &binding.scope)?;
    let result = inspect(
        options,
        snapshot,
        &binding.scope,
        &server,
        &mut database,
        &objects,
    );
    let closed = database.close();
    let value = result?;
    closed?;
    if runtime::verify(root, &options.runtime_dir)?.receipt != binding.receipt {
        return Err("物理验收期间运行绑定发生变化；保留收据并停止，不自动重放".into());
    }
    if snapshot {
        super::write_new(&options.receipt, &value)?;
    }
    super::print_json(
        &json!({"state": if snapshot { "present" } else { value["state"].as_str().ok_or("缺少核验状态")? }, "job_id": options.job_id.to_string()}),
    )
}

pub(super) fn validate(
    options: &FixtureArtifactOptions,
    root: &Path,
    snapshot: bool,
) -> Result<()> {
    if options.job_id <= 0 {
        return Err("导出任务 ID 必须是正 i64".into());
    }
    super::validate_path(
        &options.runtime_dir,
        root,
        LocalTestPathKind::ExistingDirectory,
        "运行目录",
    )?;
    if snapshot {
        super::validate_output_file(&options.receipt, root, "artifact 收据")?;
        if options.receipt.exists() {
            return Err("导出物理对象收据已存在，拒绝覆盖或重放".into());
        }
    } else {
        super::validate_path(
            &options.receipt,
            root,
            LocalTestPathKind::ExistingFile,
            "artifact 收据",
        )?;
    }
    Ok(())
}

trait EvidenceDatabase {
    fn record(&mut self, job_id: i64) -> Result<Value>;
    fn remaining(&mut self, job_id: i64, file_id: i64) -> Result<u64>;
    fn name(&self) -> &str;
}
trait EvidenceObjects {
    fn bytes(&self, key: &str) -> Result<Option<Vec<u8>>>;
}

fn inspect(
    options: &FixtureArtifactOptions,
    snapshot: bool,
    scope: &str,
    server: &str,
    database: &mut impl EvidenceDatabase,
    objects: &impl EvidenceObjects,
) -> Result<Value> {
    let owner = format!("ryframe-owner:v1:{scope}:object-storage:exports");
    if objects.bytes(".ryframe-owner")?.as_deref() != Some(owner.as_bytes()) {
        return Err("对象存储 ownership 不匹配".into());
    }
    if snapshot {
        let mut record = database.record(options.job_id)?;
        validate_record(&record, options.job_id)?;
        let content = objects.bytes(super::text_field(&record, "key", "文件记录")?)?;
        if content.as_ref().is_none_or(|bytes| {
            Some(bytes.len() as u64) != record["bytes"].as_u64()
                || crate::devex::runtime_native::artifacts::sha(bytes) != record["sha256"]
        }) {
            return Err("导出物理对象与文件登记的大小/摘要不匹配".into());
        }
        record["scope_id"] = json!(scope);
        record["server_uuid"] = json!(server);
        record["database"] = json!(database.name());
        return Ok(record);
    }
    let receipt = super::read_json(&options.receipt, "导出物理对象收据", 64 * 1024)?;
    super::exact_object(
        &receipt,
        &[
            "scope_id",
            "server_uuid",
            "database",
            "job_id",
            "file_id",
            "bucket",
            "key",
            "sha256",
            "bytes",
        ],
        "导出物理对象收据",
    )?;
    validate_record(&receipt, options.job_id)?;
    if receipt["scope_id"] != scope
        || receipt["server_uuid"] != server
        || receipt["database"] != database.name()
    {
        return Err("删除验收收据与当前资源不匹配".into());
    }
    let file_id = identifier(super::text_field(&receipt, "file_id", "导出收据")?)?;
    let remaining = database.remaining(options.job_id, file_id)?;
    let missing = objects
        .bytes(super::text_field(&receipt, "key", "导出收据")?)?
        .is_none();
    Ok(json!({"state": if remaining == 0 && missing { "deleted" } else { "pending" }}))
}

fn validate_record(record: &Value, job_id: i64) -> Result<()> {
    if identifier(super::text_field(record, "job_id", "文件记录")?)? != job_id
        || record["bucket"] != "exports"
    {
        return Err("导出产物任务或 exports 桶绑定不匹配".into());
    }
    identifier(super::text_field(record, "file_id", "文件记录")?)?;
    objects::key(super::text_field(record, "key", "文件记录")?)?;
    super::valid_sha256(super::text_field(record, "sha256", "文件记录")?, "文件摘要")?;
    if record["bytes"].as_u64().is_none() {
        return Err("导出文件大小无效".into());
    }
    Ok(())
}

fn identifier(text: &str) -> Result<i64> {
    let number = text
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0 && value.to_string() == text);
    number.ok_or_else(|| "必须提供有效的导出 Snowflake ID".into())
}

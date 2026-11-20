//! 只跨已确认且仍受保护的前代标记意图接续。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{Inputs, Receipts, Stage, common, digest, document, empty_image, protected_file};
use crate::{Result, recovery::clone::native::lifecycle::state};

pub(super) struct Prior {
    intent: PathBuf,
    directory: Inputs,
    files: Inputs,
}

impl Prior {
    pub(super) fn load(
        receipts: &Receipts,
        expected_digest: &Value,
        verify_prefix: impl FnOnce(&Value) -> Result<()>,
    ) -> Result<(Self, Value)> {
        let name = document::text(expected_digest, "第三次重启原标记代次摘要")?;
        if name.len() != 64
            || !name
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err("第三次重启原标记代次摘要无效".into());
        }
        let root = receipts.directory.parent().ok_or("缺少标记逻辑运行目录")?;
        let backend = &receipts.backend;
        let path = common::local_directory(backend, &root.join(name), "唯一允许的原标记代次目录")?;
        if path == receipts.directory {
            return Err("Redis 单独接续未建立新的标记代次目录".into());
        }
        let directory = Inputs::directory(&path)?;
        let history = state::load(&receipts.run, true)?;
        let row = confirmed_row(state::attempts(&history)?, receipts.attempt)?;
        let number = row["number"].as_u64().ok_or("原确认阶段缺少序号")?;
        let intent = path.join("intent.json");
        let observation_path = path.join(format!("observation-{number:04}.json"));
        let controller_path = state::controller_path(&receipts.run, number);
        let result_path = state::result_path(&receipts.run, number);
        let files = Inputs::open(
            [&intent, &observation_path, &controller_path, &result_path]
                .into_iter()
                .map(|path| protected_file(backend, path))
                .collect::<Result<Vec<_>>>()?,
        )?;
        let result = common::read_bound(backend, &row["result"], "原标记确认阶段结果")?;
        let observation_binding = common::binding(&observation_path)?;
        let intent_binding = common::binding(&intent)?;
        let observation = common::read_bound(backend, &observation_binding, "原完整标记观察")?;
        let record = common::read_bound(backend, &intent_binding, "原已确认标记意图")?;
        let controller = common::binding(&controller_path)?;
        Stage::verify_intent_controller(backend, &receipts.run, number, &controller)?;
        compare_images(
            receipts,
            expected_digest,
            number,
            &result,
            [&observation_binding, &intent_binding, &controller],
            [&observation, &record],
        )?;
        verify_prefix(&record["copy_prefix"])?;
        let prior = Self {
            intent,
            directory,
            files,
        };
        prior.check()?;
        Ok((
            prior,
            json!({
                "result": row["result"],
                "observation": observation_binding,
                "intent": intent_binding,
            }),
        ))
    }

    pub(super) fn intent(&self) -> &Path {
        &self.intent
    }

    pub(super) fn check(&self) -> Result<()> {
        self.directory.check()?;
        self.files.check()
    }
}

fn confirmed_row(attempts: &[Value], current: u64) -> Result<&Value> {
    let mut accepted = attempts
        .iter()
        .filter(|row| row["stage"] == "target-cache-markers" && row["status"] == "passed");
    let row = accepted.next().ok_or("原 Redis 标记阶段没有已确认结果")?;
    if accepted.next().is_some()
        || row["mode"] != "run"
        || !row["number"]
            .as_u64()
            .is_some_and(|number| number < current)
    {
        return Err("Redis 单独接续要求此前唯一正式通过的标记写入".into());
    }
    Ok(row)
}

fn compare_images(
    receipts: &Receipts,
    expected_digest: &Value,
    number: u64,
    result: &Value,
    [observation_binding, intent_binding, controller]: [&Value; 3],
    [observation, record]: [&Value; 2],
) -> Result<()> {
    let mut stable = receipts.binding.clone();
    stable["fixture"] = record["binding"]["fixture"].clone();
    if record["binding"] != stable
        || json!(digest::plan_hash(&record["binding"]["fixture"])?) != *expected_digest
    {
        return Err("原 Redis 标记绑定不同于第三次重启或现行复制计划".into());
    }
    let expected_result = json!({
        "status":"target_cache_markers_confirmed","classification":"after",
        "observation":observation_binding,"marker_write_attempts":1,
        "clone_verified":false,"restore_qualified":false,
    });
    let expected_intent = json!({
        "format_version":1,"kind":"fixture_redis_marker_intent",
        "attempt":number,"controller":controller,"binding":stable,
        "copy_prefix":record["copy_prefix"],"before":empty_image(),"after":receipts.after,
    });
    let expected_observation = json!({
        "format_version":1,"kind":"fixture_redis_marker_observation",
        "attempt":number,"controller":controller,"binding":stable,
        "intent":intent_binding,"copy_prefix":record["copy_prefix"],
        "classification":"after","observed":receipts.after,
        "observed_sha256":digest::plan_hash(&receipts.after)?,
        "namespace_key_count":document::array(&receipts.after["keys"], "当前双标记后像")?.len(),
        "marker_write_attempts":1,"confirmed":true,"remote_writes":1,
        "clone_verified":false,"restore_qualified":false,
    });
    if *result != expected_result
        || *record != expected_intent
        || *observation != expected_observation
    {
        return Err("原 Redis 标记未完整确认同一意图、控制器与初始化后像".into());
    }
    Ok(())
}

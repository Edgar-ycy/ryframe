//! 绑定原来源账本的实际字节，允许在同一账本上验证已收尾的追加。
use super::{Bound, common, document, value_path};
use crate::{
    Result,
    recovery::clone::native::lifecycle::{identity, state},
};
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path};

pub(super) const SNAPSHOT: &str = "source-state.json";

pub(super) struct SourceHistory {
    pub(super) snapshot: document::Document,
    pub(super) directory: Value,
}

impl SourceHistory {
    pub(super) fn check(&self, backend: &Path) -> Result<()> {
        common::directory_identity(backend, &self.directory)?;
        self.snapshot.assert_unchanged()
    }
}

pub(super) fn require_ready(bound: &Bound<'_>) -> Result<()> {
    read_ready(bound).map(|_| ())
}

fn read_ready<'a>(bound: &'a Bound<'_>) -> Result<(&'a Path, document::Document)> {
    let run = value_path(&bound.source.source["directory"])?;
    let (_, current) = document::bound_file(bound.backend, &bound.source.state, "共享导出原账本")?;
    require_source_state(run, &current.path, &current.value)?;
    Ok((run, current))
}

pub(super) fn capture(bound: &Bound<'_>, output: &Path) -> Result<SourceHistory> {
    let (run, current) = read_ready(bound)?;
    let metadata = fs::symlink_metadata(run)?;
    let (device, inode) = common::device_inode_for(run, &metadata)?;
    let directory = json!({"path":run,"device":device,"inode":inode});
    let path = output.join(SNAPSHOT);
    let mut staged = tempfile::NamedTempFile::new_in(output)?;
    staged.write_all(&current.raw)?;
    staged.as_file().sync_all()?;
    staged
        .persist_noclobber(&path)
        .map_err(|error| error.error)?;
    let snapshot = document::read(&path, bound.backend, "独立 B1 原来源状态快照")?;
    if snapshot.raw != current.raw || common::directory_identity(bound.backend, &directory)? != run
    {
        return Err("独立 B1 来源快照或原物理运行目录在发布期间变化".into());
    }
    current.assert_unchanged()?;
    bound.source.check(bound.backend)?;
    Ok(SourceHistory {
        snapshot,
        directory,
    })
}

pub(super) fn verify(bound: &Bound<'_>, intent: &Value, output: &Path) -> Result<SourceHistory> {
    let directory = &intent["source_run"];
    let run = value_path(&bound.source.source["directory"])?;
    if common::directory_identity(bound.backend, directory)? != run {
        return Err("独立 B1 ARM 原来源物理运行目录已变化".into());
    }
    let binding = document::binding_shape(&intent["source_state_snapshot"], "原来源状态快照")?;
    let original = document::binding_shape(&intent["source_state"], "原来源状态")?;
    if binding.path != output.join(SNAPSHOT)
        || binding.bytes != original.bytes
        || binding.sha256 != original.sha256
    {
        return Err("独立 B1 ARM 来源快照未绑定原账本的精确字节".into());
    }
    let (_, snapshot) = document::bound_file(
        bound.backend,
        &intent["source_state_snapshot"],
        "原来源状态快照",
    )?;
    require_source_state(run, &original.path, &snapshot.value)?;
    let current = state::historical(run, &intent["source_state"], true)?;
    if current["current"] != bound.source.state {
        return Err("独立 B1 ARM 来源账本在原历史与共享导出核对间变化".into());
    }
    let current_state = &current["state"];
    require_source_state(run, &original.path, current_state)?;
    let appended = appended(&snapshot.value, current_state)?;
    if current["historical_attempts"].as_u64()
        != Some(state::attempts(&snapshot.value)?.len() as u64)
    {
        return Err("独立 B1 ARM 来源账本匹配了不同的历史前缀".into());
    }
    for attempt in appended {
        validate_terminal_controller(run, attempt)?;
    }
    snapshot.assert_unchanged()?;
    bound.source.check(bound.backend)?;
    if common::directory_identity(bound.backend, directory)? != run {
        return Err("独立 B1 ARM 来源运行目录在核验期间变化".into());
    }
    Ok(SourceHistory {
        snapshot,
        directory: directory.clone(),
    })
}

fn require_source_state(run: &Path, state_path: &Path, value: &Value) -> Result<()> {
    if state_path != run.join("state.json") || state::running_attempt(value)?.is_some() {
        return Err("独立 B1 ARM 需要原共享导出账本没有运行中的阶段，待自然收尾后再核对".into());
    }
    Ok(())
}

fn appended<'a>(original: &Value, current: &'a Value) -> Result<&'a [Value]> {
    let original_attempts = state::attempts(original)?;
    let current_attempts = state::attempts(current)?;
    if original["format_version"] != current["format_version"]
        || original["manifest"] != current["manifest"]
        || !current_attempts.starts_with(original_attempts)
    {
        return Err("独立 B1 ARM 原账本未保留同一运行清单和逐项相同的历史阶段".into());
    }
    Ok(&current_attempts[original_attempts.len()..])
}

fn validate_terminal_controller(run: &Path, attempt: &Value) -> Result<()> {
    let number = attempt["number"]
        .as_u64()
        .ok_or("独立 B1 后续阶段缺少序号")?;
    if !matches!(attempt["status"].as_str(), Some("passed" | "failed"))
        || attempt["finished_at"].as_str().is_none_or(str::is_empty)
    {
        return Err("独立 B1 后续账本阶段尚无有效终态".into());
    }
    let mut running = attempt.clone();
    running["status"] = json!("running");
    running["finished_at"] = Value::Null;
    running["result"] = Value::Null;
    running["error_type"] = Value::Null;
    let (binding, owner) = state::controller_record(run, number, &running)?;
    identity::validate(&owner, run)?;
    if owner["manifest_sha256"] != common::binding(&run.join("manifest.json"))?["sha256"]
        || binding != common::binding(&state::controller_path(run, number))?
    {
        return Err("独立 B1 后续阶段的原控制器不属于同一固定运行".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::clone::native::{
        json as native,
        lifecycle::lock::{RunLock, owner_observation},
    };

    #[test]
    fn independent_b1_source_history_rechecks_the_formal_appended_controller() -> Result<()> {
        let backend = crate::workspace::root_dir();
        let fixtures = backend.join(".local-tests");
        fs::create_dir_all(&fixtures)?;
        let fixture = tempfile::tempdir_in(&fixtures)?;
        let run = fixture.path();
        native::write_new(&run.join("manifest.json"), &json!({}))?;
        state::initialize(run)?;
        let snapshot = document::read(&run.join("state.json"), &backend, "测试来源原账本")?;
        let lock = RunLock::claim(run)?;
        let (_, owner) = owner_observation(run)?.ok_or("测试控制器缺失")?;
        let (number, running) = state::begin(run, "seed-runtime", "arm-input", &json!({}))?;
        state::bind_controller(run, number, &owner, &running)?;
        state::finish(
            run,
            number,
            Some(&json!({"status":"seed_arm_input_published"})),
            None,
        )?;
        lock.release()?;

        let checked = state::historical(run, &document::binding(&snapshot), true)?;
        let successors = appended(&snapshot.value, &checked["state"])?;
        assert_eq!(successors.len(), 1);
        let attempt = &successors[0];
        validate_terminal_controller(run, attempt)?;
        let mut altered = attempt.clone();
        altered["sources"] = json!({"substituted":true});
        assert!(validate_terminal_controller(run, &altered).is_err());
        Ok(())
    }

    #[test]
    fn independent_b1_source_history_requires_an_unchanged_closed_prefix() -> Result<()> {
        let original =
            json!({"format_version":1,"manifest":"run","attempts":[{"status":"passed"}]});
        let mut successor = original.clone();
        let run = Path::new("run");
        let state_path = run.join("state.json");
        require_source_state(run, &state_path, &original)?;
        assert!(appended(&original, &successor)?.is_empty());
        successor["attempts"]
            .as_array_mut()
            .unwrap()
            .push(json!({"status":"passed"}));
        assert_eq!(appended(&original, &successor)?.len(), 1);
        let mut amended = successor.clone();
        amended["attempts"][0]["status"] = json!("failed");
        assert!(appended(&original, &amended).is_err());
        let mut other = successor.clone();
        other["manifest"] = json!("other");
        assert!(appended(&original, &other).is_err());
        successor["attempts"][1]["status"] = json!("running");
        assert!(require_source_state(run, &state_path, &successor).is_err());
        assert!(require_source_state(run, &run.join("different.json"), &original).is_err());
        Ok(())
    }
}

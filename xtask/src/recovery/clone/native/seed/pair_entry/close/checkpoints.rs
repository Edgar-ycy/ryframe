//! 保存同一 close 运行的阶段检查点，并核对历史续作来源。
use super::super::Frame;
use super::recovered;
use crate::{
    Result,
    recovery::clone::native::{
        digest,
        lifecycle::state,
        resources::common,
        seed::protocol::guards::{checkpoint, result},
        target_verify::publication,
    },
};
use serde_json::{Value, json};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

const PHASES: [(&str, &str); 5] = [
    ("running", "close-10-running.json"),
    ("api_stopped", "close-20-api-stopped.json"),
    ("drained", "close-30-drained.json"),
    ("worker_stopped", "close-40-worker-stopped.json"),
    ("verified", "close-50-verified.json"),
];

pub(super) enum Prior {
    Fresh,
    Missing,
    Resume { value: Value, binding: Value },
}

pub(super) struct Checkpoints<'a, 'b> {
    frame: &'a Frame<'b>,
}

struct Chain {
    phase: usize,
    value: Value,
    files: Vec<Value>,
    source: Option<Value>,
}

impl Chain {
    fn binding(&self) -> &Value {
        self.files.last().expect("非空 close 检查点链")
    }
}

impl<'a, 'b> Checkpoints<'a, 'b> {
    pub(super) fn new(frame: &'a Frame<'b>) -> Self {
        Self { frame }
    }

    pub(super) fn previous(&self) -> Result<Prior> {
        let (_, earlier) = self.attempts()?;
        let Some((row, intervening)) = previous_close(earlier)? else {
            return Ok(Prior::Fresh);
        };
        let prior = if row["result"].is_null() {
            match self.chain(row)? {
                Some(chain) => {
                    self.verify_source(earlier, number(row)?, chain.source.as_ref())?;
                    self.check_files(&chain.files)?;
                    Prior::Resume {
                        binding: chain.binding().clone(),
                        value: chain.value,
                    }
                }
                None => Prior::Missing,
            }
        } else {
            let value = self.outer_result(row)?;
            if row["status"] == "passed" {
                if value["status"] != "seed_runtime_closed" {
                    return Err("已通过的 close 结果状态无效".into());
                }
                return Err("seed runtime 已完成 close，禁止重复停止".into());
            }
            Prior::Resume {
                value,
                binding: row["result"].clone(),
            }
        };
        match &prior {
            Prior::Resume { value, .. } => {
                recovered::verify(self.frame, row, value, intervening, self.frame.number)?;
            }
            Prior::Missing if !intervening.is_empty() => {
                return Err("恢复间隔缺少原 Close 阶段结果或完整检查点".into());
            }
            _ => {}
        }
        self.frame.check()?;
        Ok(prior)
    }

    pub(super) fn publish(&self, result: &Value, resumed_from: Option<&Value>) -> Result<()> {
        let (current, earlier) = self.attempts()?;
        let index = PHASES
            .iter()
            .position(|(phase, _)| result["phase"].as_str() == Some(*phase))
            .ok_or("close checkpoint 阶段无效")?;
        let chain = self.chain(current)?;
        if let Some(chain) = &chain {
            if resumed_from.is_some() || index != chain.phase + 1 {
                return Err("close checkpoint 阶段必须连续且不能更换续作来源".into());
            }
            self.verify_source(earlier, self.frame.number, chain.source.as_ref())?;
            self.check_files(&chain.files)?;
        } else {
            self.verify_source(earlier, self.frame.number, resumed_from)?;
        }
        let previous = chain.as_ref().map(Chain::binding);
        let (phase, filename) = PHASES[index];
        let directory = self.directory(self.frame.number);
        common::local_directory(self.frame.backend, &directory, "当前 close 证据目录")?;
        let mut value = json!({
            "format_version":1,"kind":"devex-clone-seed-close-checkpoint",
            "attempt":self.frame.number,"controller":self.frame.controller,
            "handoff":self.frame.handoff,"runtime_directory":self.frame.runtime.runtime,
            "scope_id":self.frame.runtime.handoff["scope_id"],"phase":phase,
            "previous":previous,"resumed_from":resumed_from,"result":result
        });
        let hash = digest::plan_hash(&value)?;
        value["sha256"] = json!(hash);
        self.validate(current, &value, phase, previous)?;
        self.frame.check()?;
        let path = directory.join(filename);
        publication::publish(&path, &value)?;
        let binding = self.binding(&path)?;
        if self.read(&binding)? != value {
            return Err("新 close checkpoint 发布后字节变化".into());
        }
        self.frame.check()
    }

    pub(super) fn authorize_stop(&self, phase: usize) -> Result<(Value, Vec<Value>)> {
        let (current, earlier) = self.attempts()?;
        let mut chain = self.chain(current)?.ok_or("关闭停止缺少当前检查点")?;
        if chain.phase != phase {
            return Err("关闭停止必须属于当前精确阶段，禁止重发".into());
        }
        let ancestors = self.verify_source(earlier, self.frame.number, chain.source.as_ref())?;
        chain.files.extend(ancestors);
        self.check_files(&chain.files)?;
        Ok((chain.value, chain.files))
    }

    fn attempts(&self) -> Result<(&Value, &[Value])> {
        self.frame.check()?;
        let (current, earlier) = state::attempts(&self.frame.saved)?
            .split_last()
            .ok_or("close 缺少当前统一验收阶段")?;
        if !is_close(current)
            || current["status"] != "running"
            || number(current)? != self.frame.number
        {
            return Err("close checkpoint 只属于当前持锁 attempt".into());
        }
        Ok((current, earlier))
    }

    fn outer_result(&self, row: &Value) -> Result<Value> {
        let binding = &row["result"];
        let path = common::bound_file(self.frame.backend, binding, "原 close 外层结果")?;
        if path != state::result_path(self.frame.run, number(row)?) {
            return Err("close 外层结果不在该原 attempt 的固定位置".into());
        }
        let value = self.read(binding)?;
        result::validate(
            &value,
            &self.frame.runtime.runtime,
            &self.frame.runtime.handoff,
            &self.frame.handoff,
            false,
        )?;
        common::bound_file(self.frame.backend, binding, "原 close 外层结果后像")?;
        Ok(value)
    }

    fn chain(&self, row: &Value) -> Result<Option<Chain>> {
        if !is_close(row) {
            return Err("close checkpoint 不属于 close attempt".into());
        }
        let directory = self.directory(number(row)?);
        let mut last = None::<Chain>;
        for (index, (phase, name)) in PHASES.into_iter().enumerate() {
            let path = directory.join(name);
            match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
                Ok(_) => (),
            }
            let binding = self.binding(&path)?;
            let value = self.read(&binding)?;
            let previous = last.as_ref().map(Chain::binding);
            if last.as_ref().is_some_and(|chain| index != chain.phase + 1) {
                return Err("close checkpoint 阶段链不连续".into());
            }
            let result = self.validate(row, &value, phase, previous)?;
            match &mut last {
                Some(chain) => {
                    chain.phase = index;
                    chain.value = result;
                    chain.files.push(binding);
                }
                None => {
                    last = Some(Chain {
                        phase: index,
                        value: result,
                        files: vec![binding],
                        source: value
                            .get("resumed_from")
                            .filter(|item| !item.is_null())
                            .cloned(),
                    });
                }
            }
        }
        if let Some(chain) = &last {
            self.check_files(&chain.files)?;
        }
        Ok(last)
    }

    fn verify_source(
        &self,
        earlier: &[Value],
        mut before: u64,
        source: Option<&Value>,
    ) -> Result<Vec<Value>> {
        let mut next = source.cloned();
        let mut files = Vec::new();
        while let Some(binding) = next {
            let path = common::bound_file(self.frame.backend, &binding, "原 close 续作来源")?;
            let (row, attempt) = self.source_attempt(earlier, before, &path)?;
            if path == state::result_path(self.frame.run, attempt) {
                if row["result"] != binding {
                    return Err("close 续作来源不是原外层发布结果".into());
                }
                let value = self.outer_result(row)?;
                files.extend(recovered::verify(
                    self.frame,
                    row,
                    &value,
                    between(earlier, attempt, before),
                    before,
                )?);
                files.push(binding);
                break;
            }
            let chain = self.chain(row)?.ok_or("原 close 续作来源缺少检查点链")?;
            if *chain.binding() != binding {
                return Err("close 续作来源不是原 attempt 的最新完整链".into());
            }
            files.extend(recovered::verify(
                self.frame,
                row,
                &chain.value,
                between(earlier, attempt, before),
                before,
            )?);
            before = attempt;
            next = chain.source;
            files.extend(chain.files);
        }
        self.check_files(&files)?;
        Ok(files)
    }

    fn source_attempt<'c>(
        &self,
        earlier: &'c [Value],
        before: u64,
        path: &Path,
    ) -> Result<(&'c Value, u64)> {
        for row in earlier.iter().rev().filter(|row| is_close(row)) {
            let attempt = number(row)?;
            if attempt < before
                && (path == state::result_path(self.frame.run, attempt)
                    || PHASES
                        .iter()
                        .any(|(_, name)| path == self.directory(attempt).join(name)))
            {
                return Ok((row, attempt));
            }
        }
        Err("close checkpoint 续作来源不属于更早的固定 close".into())
    }

    fn validate(
        &self,
        row: &Value,
        value: &Value,
        phase: &str,
        previous: Option<&Value>,
    ) -> Result<Value> {
        checkpoint::validate(
            self.frame.backend,
            &json!({
                "directory":self.frame.run,"attempt":row,"value":value,
                "phase":phase,"previous":previous,"runtime":self.frame.runtime.runtime,
                "handoff":self.frame.runtime.handoff
            }),
        )
    }

    fn directory(&self, attempt: u64) -> PathBuf {
        self.frame
            .run
            .join("seed-runtime")
            .join(format!("attempt-{attempt:04}"))
    }

    fn binding(&self, path: &Path) -> Result<Value> {
        common::binding(&common::common_file(
            self.frame.backend,
            path,
            "close 检查点",
        )?)
    }

    fn read(&self, binding: &Value) -> Result<Value> {
        let value = common::read_bound(self.frame.backend, binding, "close 固定证据")?;
        common::bound_file(self.frame.backend, binding, "close 固定证据后像")?;
        Ok(value)
    }

    fn check_files(&self, files: &[Value]) -> Result<()> {
        for binding in files {
            common::bound_file(self.frame.backend, binding, "close 原始摘要链后像")?;
        }
        Ok(())
    }
}

fn is_close(row: &Value) -> bool {
    row["stage"] == "seed-runtime" && row["mode"] == "close"
}

fn number(row: &Value) -> Result<u64> {
    row["number"]
        .as_u64()
        .filter(|number| *number > 0)
        .ok_or_else(|| "close 阶段序号无效".into())
}

fn between(earlier: &[Value], after: u64, before: u64) -> &[Value] {
    let start = earlier.partition_point(|row| row["number"].as_u64().is_some_and(|n| n <= after));
    let end = earlier.partition_point(|row| row["number"].as_u64().is_some_and(|n| n < before));
    &earlier[start..end]
}

fn previous_close(earlier: &[Value]) -> Result<Option<(&Value, &[Value])>> {
    let Some(index) = earlier.iter().rposition(is_close) else {
        return Ok(None);
    };
    let intervening = &earlier[index + 1..];
    if intervening.iter().any(|row| !recovered::candidate(row)) {
        return Err("上次 close 后已有其他阶段，不能猜测进程或任务状态".into());
    }
    Ok(Some((&earlier[index], intervening)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_close_selects_passed_or_explicitly_interrupted_recovery() -> Result<()> {
        let start = json!({"number":1,"stage":"seed-runtime","mode":"start"});
        let first = json!({"number":2,"stage":"seed-runtime","mode":"close","result":null});
        let latest = json!({"number":3,"stage":"seed-runtime","mode":"close","status":"failed"});
        let recover = json!({"number":4,"stage":"seed-runtime","mode":"recover","status":"passed"});
        assert!(previous_close(std::slice::from_ref(&start))?.is_none());
        let earlier = [start, first, latest.clone(), recover];
        assert_eq!(previous_close(&earlier[..3])?, Some((&latest, &[][..])));
        assert_eq!(previous_close(&earlier)?, Some((&latest, &earlier[3..])));
        assert_eq!(between(&earlier, 3, 5), &earlier[3..]);
        let mut rejected = earlier;
        rejected[3]["status"] = json!("failed");
        assert!(previous_close(&rejected).is_err());
        rejected[3]["error_type"] = json!("ControllerInterrupted");
        rejected[3]["finished_at"] = json!("2026-11-09T02:00:00Z");
        rejected[3]["result"] = Value::Null;
        assert_eq!(previous_close(&rejected)?, Some((&latest, &rejected[3..])));
        rejected[3]["mode"] = json!("stop");
        rejected[3]["status"] = json!("passed");
        assert!(previous_close(&rejected).is_err());
        Ok(())
    }
}

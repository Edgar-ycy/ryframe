use super::super::history;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{json as json_io, lifecycle::state},
};
use serde_json::Value;
use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(in crate::recovery::fixture_control::services) struct LifecycleError {
    pub(super) category: &'static str,
    pub(in crate::recovery::fixture_control::services) message: &'static str,
}
impl fmt::Display for LifecycleError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(self.message)
    }
}
impl Error for LifecycleError {}
pub(super) fn error(message: &'static str) -> Box<dyn Error> {
    Box::new(LifecycleError {
        category: "LifecycleRejected",
        message,
    })
}
pub(super) fn check(valid: bool, message: &'static str) -> Result<()> {
    if valid { Ok(()) } else { Err(error(message)) }
}
pub(super) fn reason(error: &(dyn Error + 'static)) -> Option<&'static str> {
    error
        .downcast_ref::<LifecycleError>()
        .map(|error| error.message)
        .or_else(|| super::super::storage::rejection_reason(error))
}
pub(super) fn category(error: &(dyn Error + 'static)) -> &'static str {
    error
        .downcast_ref::<LifecycleError>()
        .map_or("LifecycleFailure", |value| value.category)
}
pub(super) fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| error("生命周期输入缺少必要字符串字段"))
}
pub(super) fn binding(path: &Path) -> Result<Value> {
    super::super::super::file_binding(path, "生命周期绑定")
}
pub(super) fn read(path: &Path) -> Result<Value> {
    check(
        !crate::recovery::clone::native::digest::linked_path(path)?.contains(&true),
        "生命周期文件经过链接",
    )?;
    json_io::read(path)
}

pub(super) struct Context {
    pub(super) backend: PathBuf,
    pub(super) run: PathBuf,
    pub(super) value: Value,
}
impl Context {
    pub(super) fn load(backend: &Path, value: Value) -> Result<Self> {
        let run = PathBuf::from(text(&value, "run")?);
        validate_local_test_path(&run, backend, LocalTestPathKind::ExistingDirectory)?;
        check(
            Path::new(text(&value, "backend")?) == backend,
            "生命周期上下文属于其他后端",
        )?;
        history::validate_manifest(&value["manifest"])?;
        check(
            read(&run.join("manifest.json"))? == value["manifest"],
            "生命周期固定清单变化",
        )?;
        let context = Self {
            backend: backend.to_path_buf(),
            run,
            value,
        };
        context.guard(None)?;
        history::validate(&context.run, &context.value["state"], false, false)?;
        Ok(context)
    }
    pub(super) fn sources(&self) -> Value {
        self.value["sources"].clone()
    }
    pub(super) fn history(&self, settled: bool) -> Result<Value> {
        history::validate(&self.run, &self.value["state"], true, settled)
    }
    pub(super) fn guard(&self, running: Option<&Value>) -> Result<()> {
        let sources = self.value["sources"]
            .as_object()
            .ok_or("生命周期缺少来源绑定")?;
        for (key, descriptor) in sources {
            let expected = if key == "state_before" {
                running.unwrap_or(descriptor)
            } else {
                descriptor
            };
            let path = crate::recovery::clone::native::lifecycle::manifest::bound_file(
                &self.backend,
                expected,
                "生命周期来源",
                true,
            )?;
            check(
                binding(&path)? == *expected,
                "生命周期来源在执行期间发生变化",
            )?;
            if let Some(document) = self.value["documents"].get(key) {
                check(
                    read(&path)? == *document,
                    "生命周期输入内容与采样摘要不一致",
                )?;
            }
        }
        if running.is_none() {
            check(
                state::load(&self.run, true)? == self.value["state"],
                "生命周期账本在采样期间变化",
            )?;
        }
        let environment = Path::new(text(&self.value["sources"]["environment"], "path")?);
        check(
            read(environment)?["environment"] == self.value["private"],
            "生命周期私有环境在执行期间变化",
        )
    }
    pub(super) fn owner(&self, selected: &Path) -> Result<PathBuf> {
        let path = if selected.is_absolute() {
            selected.to_path_buf()
        } else {
            self.backend.join(selected)
        };
        validate_local_test_path(&path, &self.backend, LocalTestPathKind::ExistingFile)?;
        Ok(path)
    }
    pub(super) fn require_current_owner(&self, selected: &Path) -> Result<()> {
        let path = self.owner(selected)?;
        let recorded = Path::new(text(&self.value["sources"]["state_before"], "path")?);
        check(
            crate::local_test_path::same_local_test_file(
                &self.backend,
                recorded,
                &self.run.join("state.json"),
            )? && crate::local_test_path::same_local_test_file(&self.backend, &path, recorded)?
                && binding(recorded)? == self.value["sources"]["state_before"],
            "生命周期操作必须精确绑定 status 返回的当前 state 文件",
        )
    }
    pub(super) fn unknown(&self) -> Result<Vec<String>> {
        let state = state::load(&self.run, true)?;
        let attempts = state::attempts(&state)?;
        let mut consumed = Vec::new();
        let mut expected = Vec::new();
        for item in attempts {
            if item["stage"] != "fixture-services" {
                continue;
            }
            let number = item["number"].as_u64().ok_or("生命周期编号无效")?;
            if [
                "close",
                "reconcile",
                "restart",
                "restart-redis",
                "handoff-rustfs",
            ]
            .iter()
            .any(|mode| item["mode"] == *mode)
            {
                expected.push(format!("lifecycle-{number:04}"));
            }
            if item["mode"] == "recover" && item["status"] == "passed" {
                let result = read(Path::new(text(&item["result"], "path")?))?;
                if let Some(path) = result["recovery"]["path"].as_str() {
                    consumed.push(PathBuf::from(path));
                }
                if result["status"] == "external_termination_reconciled" {
                    expected.push(format!("lifecycle-{number:04}"));
                }
            }
        }
        let mut unknown = Vec::new();
        for item in std::fs::read_dir(&self.run)? {
            let item = item?;
            let name = item
                .file_name()
                .into_string()
                .map_err(|_| "生命周期文件名不是 UTF-8")?;
            let unfinished = name
                .strip_prefix("recovery-")
                .and_then(|name| name.strip_suffix(".intent.json"))
                .is_some_and(|id| !self.run.join(format!("recovered-{id}.json")).is_file());
            if unfinished
                || name.starts_with("recovered-")
                    && name.ends_with(".json")
                    && !consumed.contains(&item.path())
                || name.starts_with("lifecycle-") && !expected.contains(&name)
            {
                unknown.push(name);
            }
        }
        unknown.sort();
        unknown.dedup();
        Ok(unknown)
    }
    pub(super) fn no_unknown(&self) -> Result<()> {
        check(
            self.unknown()?.is_empty(),
            "存在未知控制恢复或生命周期证据；先核实完整前后像",
        )
    }
}

//! 原失败和唯一报告只读绑定；追加或替换报告均不能取得清理权限。
use super::super::super::super::{json as document, resources::common};
use super::super::super::guard::Inputs;
use crate::Result;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) struct Evidence {
    target: PathBuf,
    paths: Vec<PathBuf>,
    bindings: Vec<Value>,
    protected: Inputs,
    pub(super) prepared: Value,
}

impl Evidence {
    pub(super) fn open(target: &Path, request: &Value) -> Result<Self> {
        let report = unique_report(target)?;
        let paths = vec![
            target.join("failure.json"),
            report,
            target.join("prepare.json"),
            target.join("initialize.started.json"),
        ];
        let protected = Inputs::open(paths.clone())?;
        let failure = document::read(&paths[0])?;
        let report = document::read(&paths[1])?;
        eligible(&failure, &report)?;
        let prepared = document::read(&paths[2])?;
        if prepared["request"] != *request {
            return Err("收尾请求不同于固定 registration".into());
        }
        let bindings = paths
            .iter()
            .map(|p| common::binding(p))
            .collect::<Result<_>>()?;
        let value = Self {
            target: target.to_owned(),
            paths,
            bindings,
            protected,
            prepared,
        };
        value.check()?;
        Ok(value)
    }

    pub(super) fn check(&self) -> Result<()> {
        self.protected.check()?;
        if unique_report(&self.target)? != self.paths[1] {
            return Err("收尾 reset 报告集合发生变化".into());
        }
        for (path, expected) in self.paths.iter().zip(&self.bindings) {
            if common::binding(path)? != *expected {
                return Err("收尾固定失败、报告或准备证据发生变化".into());
            }
        }
        Ok(())
    }

    pub(super) fn failure_digest(&self) -> Value {
        digest(&self.bindings[0])
    }
    pub(super) fn report_digest(&self) -> Value {
        digest(&self.bindings[1])
    }
}

fn digest(binding: &Value) -> Value {
    json!({"bytes":binding["bytes"],"sha256":binding["sha256"]})
}

fn unique_report(target: &Path) -> Result<PathBuf> {
    let mut reports = fs::read_dir(target.join("reset-state"))?
        .map(|entry| Ok(entry?.path()))
        .collect::<Result<Vec<_>>>()?;
    reports.retain(|path| {
        path.file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|v| v.ends_with(".report.json"))
    });
    if reports.len() != 1 {
        return Err("reset preflight 收尾需要唯一报告".into());
    }
    Ok(reports.remove(0))
}

fn eligible(failure: &Value, report: &Value) -> Result<()> {
    document::exact(
        failure,
        &[
            "status",
            "stage",
            "at",
            "error_type",
            "reason",
            "automatic_retry",
            "automatic_resource_cleanup",
            "fresh_target_initialized",
            "clone_verified",
            "restore_qualified",
        ],
        "初始化失败",
    )?;
    if failure["stage"] != "create_and_reset" || failure["error_type"] != "CalledProcessError" {
        return Err("仅允许收尾已记录的 reset preflight 失败".into());
    }
    let phases = document::object(&report["phases"], "reset 阶段")?;
    if report["status"] != "failed"
        || report["failed_phase"] != "preflight"
        || phases
            .get("preflight")
            .is_none_or(|v| v["status"] != "failed")
        || phases
            .get("release")
            .is_none_or(|v| v["status"] != "complete")
        || phases.iter().any(|(name, value)| {
            !matches!(name.as_str(), "preflight" | "release") && value["status"] != "pending"
        })
    {
        return Err("reset 已进入资源写入或报告不属于可收尾的 preflight 失败".into());
    }
    Ok(())
}

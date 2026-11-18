//! 复用原复制目录和代次记录，将离线计划绑定到当前已观察的两侧。
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::super::{
    digest,
    input::digest_shape,
    json as document,
    resources::common,
    target_verify::{guard::Inputs, publication},
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, same_local_test_file, validate_local_test_path},
};

const SESSION_FIELDS: &[&str] = &[
    "source_export",
    "initialized",
    "generation_sha256",
    "plan_sha256",
    "source_observation",
    "clone_verified",
    "restore_qualified",
];

pub(in crate::recovery::clone::native) struct Files<'a> {
    backend: &'a Path,
    pub(in crate::recovery::clone::native) root: PathBuf,
    pub(in crate::recovery::clone::native) source_export: &'a Value,
    pub(in crate::recovery::clone::native) initialized: &'a Value,
    source_environment: BTreeMap<String, String>,
    target_environment: BTreeMap<String, String>,
    protected: Inputs,
}

/// 内容来自调用方已观察的原导出、目标历史和当前来源库存。
pub(in crate::recovery::clone::native) struct Facts<'a> {
    pub(in crate::recovery::clone::native) source_generation: &'a Value,
    pub(in crate::recovery::clone::native) target_generation: &'a Value,
    pub(in crate::recovery::clone::native) source_observation: &'a Value,
    pub(in crate::recovery::clone::native) declaration: &'a Value,
}

pub(in crate::recovery::clone::native) struct Prepared {
    pub(in crate::recovery::clone::native) plan: Value,
    pub(in crate::recovery::clone::native) record: Value,
    protected: Inputs,
}

pub(super) struct Continuation {
    pub(super) record: Value,
    wrapper: Value,
    pub(super) declaration: Value,
    plan_files: Inputs,
    session_file: Inputs,
}

impl Continuation {
    pub(super) fn check(&self) -> Result<()> {
        self.session_file.check()?;
        self.plan_files.check()
    }
}

impl<'a> Files<'a> {
    pub(in crate::recovery::clone::native) fn bind(
        backend: &'a Path,
        manifest: &'a Value,
        source_export: &'a Value,
    ) -> Result<Self> {
        if !manifest["source_export"].is_null() && manifest["source_export"] != *source_export {
            return Err("复制来源不是本运行清单固定的原导出".into());
        }
        let root = common::local_directory(
            backend,
            Path::new(document::text(&manifest["copy_directory"], "固定复制目录")?),
            "固定复制目录",
        )?;
        let initialized = &manifest["initialized"];
        let export = common::bound_file(backend, source_export, "原复制来源")?;
        let target = common::bound_file(backend, initialized, "原目标初始化")?;
        let source_env = &manifest["source_environment"];
        let target_env = &manifest["target_environment"];
        let source_path = common::bound_file(backend, source_env, "来源私有环境")?;
        let target_path = common::bound_file(backend, target_env, "目标私有环境")?;
        let protected = Inputs::open([export, target, source_path, target_path])?;
        common::bound_file(backend, source_export, "原复制来源")?;
        common::bound_file(backend, initialized, "原目标初始化")?;
        let source_environment = environment(backend, source_env, "来源私有环境")?;
        let target_environment = environment(backend, target_env, "目标私有环境")?;
        protected.check()?;
        Ok(Self {
            backend,
            root,
            source_export,
            initialized,
            source_environment,
            target_environment,
            protected,
        })
    }

    pub(in crate::recovery::clone::native) fn check(&self) -> Result<()> {
        self.protected.check()
    }

    pub(in crate::recovery::clone::native) fn environments(
        &self,
    ) -> (&BTreeMap<String, String>, &BTreeMap<String, String>) {
        (&self.source_environment, &self.target_environment)
    }

    pub(in crate::recovery::clone::native) fn read_record(&self) -> Result<Value> {
        self.check()?;
        let path =
            common::common_file(self.backend, &self.root.join("session.json"), "原复制会话")?;
        let protected = Inputs::open([path.clone()])?;
        let record = document::read(&path)?;
        document::exact(&record, SESSION_FIELDS, "原复制会话")?;
        digest_shape(&record["generation_sha256"])?;
        digest_shape(&record["plan_sha256"])?;
        if record["source_export"] != *self.source_export
            || record["initialized"] != *self.initialized
            || record["clone_verified"] != false
            || record["restore_qualified"] != false
            || record["source_observation"]
                .as_object()
                .is_none_or(|observations| observations.len() != 4)
        {
            return Err("原复制会话的来源、初始化、来源库存或资格字段不同".into());
        }
        protected.check()?;
        self.check()?;
        Ok(record)
    }

    pub(super) fn continuation(&self, stage: &str) -> Result<Continuation> {
        let session =
            common::common_file(self.backend, &self.root.join("session.json"), "原复制会话")?;
        let session_file = Inputs::open([session])?;
        let record = self.read_record()?;
        let (wrapper, declaration, plan_files) = self.load_plan()?;
        let mut plan = document::object(&wrapper["plan"], "原完整计划投影")?.clone();
        let recorded = plan.remove("plan_sha256").ok_or("原计划缺少摘要")?;
        if recorded != record["plan_sha256"]
            || digest_shape(&recorded)? != digest::plan_hash(&Value::Object(plan))?
            || wrapper["plan"]["input_sha256"] != digest::plan_hash(&declaration)?
            || wrapper["plan"]["copy_stage"] != stage
            || declaration["copy_stage"] != stage
        {
            return Err("原计划、固定输入或复制阶段与复制会话绑定不同".into());
        }
        let continuation = Continuation {
            record,
            wrapper,
            declaration,
            plan_files,
            session_file,
        };
        continuation.check()?;
        self.check()?;
        Ok(continuation)
    }

    pub(in crate::recovery::clone::native) fn prepare(&self, facts: Facts<'_>) -> Result<Prepared> {
        self.check()?;
        let input = self.root.join("input.json");
        let plan = self.root.join("plan.json");
        let session = self.root.join("session.json");
        for path in [&input, &plan, &session] {
            validate_local_test_path(path, self.backend, LocalTestPathKind::OutputFile)?;
            if path.try_exists()? {
                return Err("复制目录已有准备产物，保留原证据并显式核对".into());
            }
        }
        publication::publish(&input, facts.declaration)?;
        let input_guard = Inputs::open([input.clone()])?;
        let summary = super::super::plan(self.backend, &input, &plan)?;
        let (wrapper, plan_guard) = self.read_plan(facts.declaration)?;
        if summary["plan_sha256"] != wrapper["plan"]["plan_sha256"] {
            return Err("签发的离线计划在准备期间变化".into());
        }
        let record = self.value(&facts, &wrapper["plan"])?;
        input_guard.check()?;
        plan_guard.check()?;
        self.check()?;
        publication::publish(&session, &record)?;
        let prepared = self.prepared(wrapper, record)?;
        plan_guard.check()?;
        Ok(prepared)
    }

    pub(super) fn hydrate(&self, saved: &Continuation, facts: Facts<'_>) -> Result<Prepared> {
        self.check()?;
        saved.check()?;
        if self.read_record()? != saved.record || saved.declaration != *facts.declaration {
            return Err("复原期间原复制会话记录或完整声明变化".into());
        }
        super::super::issue(self.backend, &self.root.join("plan.json"), &saved.wrapper)?;
        if self.value(&facts, &saved.wrapper["plan"])? != saved.record {
            return Err("原复制代次、已登记环境、计划或当前来源库存变化".into());
        }
        let prepared = self.prepared(saved.wrapper.clone(), saved.record.clone())?;
        saved.check()?;
        Ok(prepared)
    }

    pub(super) fn read_plan(&self, expected: &Value) -> Result<(Value, Inputs)> {
        let (wrapper, declaration, guard) = self.load_plan()?;
        if declaration != *expected {
            return Err("原离线计划没有绑定完整当前声明".into());
        }
        Ok((wrapper, guard))
    }

    fn load_plan(&self) -> Result<(Value, Value, Inputs)> {
        let plan_path =
            common::common_file(self.backend, &self.root.join("plan.json"), "原完整计划")?;
        let input_path =
            common::common_file(self.backend, &self.root.join("input.json"), "原计划输入")?;
        let guard = Inputs::open([plan_path.clone(), input_path.clone()])?;
        let wrapper = document::read(&plan_path)?;
        document::exact(
            &wrapper,
            &["format_version", "input_path", "input_file", "plan"],
            "原离线计划",
        )?;
        let recorded = Path::new(document::text(&wrapper["input_path"], "原计划输入")?);
        let binding = digest::file(&input_path)?;
        if wrapper["format_version"] != 1
            || !same_local_test_file(self.backend, recorded, &input_path)?
            || wrapper["input_file"] != json!({"bytes":binding.bytes,"sha256":binding.sha256})
        {
            return Err("原离线计划没有绑定固定复制目录中的输入原字节".into());
        }
        let declaration = document::read(&input_path)?;
        guard.check()?;
        Ok((wrapper, declaration, guard))
    }

    pub(super) fn value(&self, facts: &Facts<'_>, plan: &Value) -> Result<Value> {
        let plan = digest_shape(&plan["plan_sha256"])?;
        let environment = json!({
            "source": digest::plan_hash(&serde_json::to_value(&self.source_environment)?)?,
            "target": digest::plan_hash(&serde_json::to_value(&self.target_environment)?)?,
        });
        let generation = digest::plan_hash(&json!({
            "source": self.source_export, "target": self.initialized,
            "source_generation": facts.source_generation, "target_generation": facts.target_generation,
            "environments": environment, "plan": plan,
        }))?;
        if facts
            .source_observation
            .as_object()
            .is_none_or(|observations| observations.len() != 4)
        {
            return Err("复制会话必须绑定来源四库完整库存".into());
        }
        Ok(json!({
            "source_export":self.source_export, "initialized":self.initialized,
            "generation_sha256":generation, "plan_sha256":plan,
            "source_observation":facts.source_observation,
            "clone_verified":false, "restore_qualified":false,
        }))
    }

    fn prepared(&self, wrapper: Value, record: Value) -> Result<Prepared> {
        let protected = Inputs::open(
            ["input.json", "plan.json", "session.json"].map(|name| self.root.join(name)),
        )?;
        if self.read_record()? != record || document::read(&self.root.join("plan.json"))? != wrapper
        {
            return Err("复制会话文件在建立保护期间变化".into());
        }
        protected.check()?;
        Ok(Prepared {
            plan: wrapper["plan"].clone(),
            record,
            protected,
        })
    }
}

impl Prepared {
    pub(in crate::recovery::clone::native) fn check(&self) -> Result<()> {
        self.protected.check()
    }
}

fn environment(
    backend: &Path,
    descriptor: &Value,
    label: &str,
) -> Result<BTreeMap<String, String>> {
    let value = common::read_bound(backend, descriptor, label)?;
    document::exact(&value, &["environment"], "复制私有环境")?;
    let mut names = BTreeSet::new();
    let values = document::object(&value["environment"], "复制私有进程环境")?
        .iter()
        .map(|(key, value)| {
            if key.is_empty()
                || key.contains(['=', '\0'])
                || !names.insert(key.to_ascii_uppercase())
            {
                return Err("复制私有环境字段无效或大小写重复".into());
            }
            let value = value
                .as_str()
                .filter(|value| !value.contains('\0'))
                .ok_or("复制私有环境值必须为有效文本")?;
            Ok((key.clone(), value.to_owned()))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    if values.get("APP_ENV").map(String::as_str) != Some("test") {
        return Err("复制两侧必须明确使用 APP_ENV=test".into());
    }
    Ok(values)
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/copy_transfer/record.rs"]
mod tests;

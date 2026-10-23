use super::super::{plan, producer_authorization};
use crate::{
    Result,
    recovery::clone::native::{digest, json as document},
};
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(super) struct Registration {
    pub run: PathBuf,
    pub attempt: u64,
    pub family: &'static str,
    pub mode: String,
    pub plan_path: PathBuf,
    pub plan_sha256: String,
}

impl Registration {
    pub(super) fn parse(arguments: &[String], root: &Path) -> Result<Self> {
        if arguments.len() != 10 {
            return Err("身份桥接生产者参数数量无效".into());
        }
        let mut values = BTreeMap::new();
        for pair in arguments.as_chunks::<2>().0 {
            if pair[1].is_empty()
                || pair[1].starts_with("--")
                || values.insert(pair[0].as_str(), pair[1].as_str()).is_some()
            {
                return Err("身份桥接生产者参数重复或缺值".into());
            }
        }
        let family = if values.contains_key("--quota-mode") {
            "quota"
        } else if values.contains_key("--department-mode") {
            "department"
        } else {
            return Err("身份桥接生产者类型无效".into());
        };
        let expected = [
            "--run-dir",
            "--attempt",
            if family == "quota" {
                "--quota-mode"
            } else {
                "--department-mode"
            },
            "--identity-plan",
            "--identity-plan-sha256",
        ];
        let actual = values
            .keys()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        let expected = expected
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        if actual != expected {
            return Err("身份桥接生产者参数集合无效".into());
        }
        let value = |key| values.get(key).copied().ok_or("身份桥接参数缺失");
        let attempt = value("--attempt")?;
        let attempt = decimal(attempt).ok_or("身份桥接 attempt 无效")?;
        let mode = value(if family == "quota" {
            "--quota-mode"
        } else {
            "--department-mode"
        })?;
        let valid = if family == "quota" {
            matches!(mode, "plan" | "apply" | "reconcile")
        } else {
            matches!(mode, "plan" | "apply" | "reconcile" | "verify")
        };
        if !valid {
            return Err("身份桥接模式无效".into());
        }
        let run = PathBuf::from(value("--run-dir")?);
        let plan_path = PathBuf::from(value("--identity-plan")?);
        crate::local_test_path::validate_local_test_path(
            &run,
            root,
            crate::local_test_path::LocalTestPathKind::ExistingDirectory,
        )?;
        crate::local_test_path::validate_local_test_path(
            &plan_path,
            root,
            crate::local_test_path::LocalTestPathKind::ExistingFile,
        )?;
        let plan_sha256 = value("--identity-plan-sha256")?.to_owned();
        if !sha256(&plan_sha256) {
            return Err("身份桥接计划摘要无效".into());
        }
        Ok(Self {
            run,
            attempt,
            family,
            mode: mode.to_owned(),
            plan_path,
            plan_sha256,
        })
    }

    pub(super) fn authorize(&self, root: &Path, grant: &Value) -> Result<Value> {
        if digest::file(&self.plan_path)?.sha256 != self.plan_sha256 {
            return Err("身份桥接计划原始文件摘要变化".into());
        }
        let registration_path = self.run.join("seed-runtime.json");
        let registration = document::read(&registration_path)?;
        let tool = &registration["identity_tool"];
        let source = root.join("xtask/src/data/performance_identities/clone_bridge.rs");
        let command = self.command(tool)?;
        let mode = format!("{}s-{}", self.family, self.mode);
        let producer_kind = format!("{}-{}", self.family, self.mode);
        let spec = producer_authorization::Spec {
            run: &self.run,
            attempt: self.attempt,
            stage: "seed-runtime",
            mode: &mode,
            producer_kind: &producer_kind,
            operation: "initialize",
            registration: &registration_path,
            source: &source,
            command: &command,
            tool,
            owner_field: "producer_owner",
        };
        let authorization = json!({
            "operation":grant["operation"],
            "run_dir":grant["run_dir"],
            "attempt":grant["attempt"],
            "producer_owner":grant["producer_owner"],
            "launch":grant["launch"],
            "process":grant["process"],
        });
        producer_authorization::authorize(root, &spec, &authorization)?;
        if registration["identity_plan"] != producer_authorization::binding(&self.plan_path)? {
            return Err("身份桥接登记未绑定当前计划".into());
        }
        let value = document::read(&self.plan_path)?;
        plan::validate(&value)?;
        let backend = Path::new(text(&value["environment"], "backend_dir")?);
        plan::load(&value, backend)?;
        if digest::file(&self.plan_path)?.sha256 != self.plan_sha256 {
            return Err("身份桥接授权期间计划变化".into());
        }
        Ok(value)
    }

    fn command(&self, tool: &Value) -> Result<Vec<String>> {
        let mut command = vec![
            text(tool, "path")?.to_owned(),
            "__identity-bridge".into(),
            "--run-dir".into(),
            self.run.to_string_lossy().into_owned(),
            "--attempt".into(),
            self.attempt.to_string(),
            format!("--{}-mode", self.family),
            self.mode.clone(),
            "--identity-plan".into(),
            self.plan_path.to_string_lossy().into_owned(),
            "--identity-plan-sha256".into(),
            self.plan_sha256.clone(),
        ];
        command.shrink_to_fit();
        Ok(command)
    }
}

pub(super) fn exact<'a>(
    value: &'a Value,
    fields: &[&str],
    label: &str,
) -> Result<&'a Map<String, Value>> {
    document::exact(value, fields, label)
}

pub(super) fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("{key} 必须为字符串").into())
}

pub(super) fn decimal(value: &str) -> Option<u64> {
    if value.is_empty()
        || value.starts_with('0')
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    value
        .parse::<u64>()
        .ok()
        .filter(|number| (1..=9_007_199_254_740_991).contains(number))
}

pub(super) fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn authorization(
    response: &Value,
    identity: &Value,
    required: &[&str],
    wildcard: bool,
) -> Result<Value> {
    let auth = &response["data"];
    let subject = text(&auth["user"], "id")?;
    decimal(subject).ok_or("授权主体 ID 无效")?;
    for (source, target) in [("tenant_id", "tenant_id"), ("username", "username")] {
        if auth["user"][target] != identity[source] {
            return Err("授权主体不同".into());
        }
    }
    if let Some(expected) = identity.get("subject_id")
        && auth["user"]["id"] != *expected
    {
        return Err("授权主体 ID 不同".into());
    }
    let permissions = auth["permissions"].as_array().ok_or("授权权限不是列表")?;
    if !auth["is_super_admin"].is_boolean()
        || permissions.iter().any(|permission| !permission.is_string())
        || (!wildcard && permissions.iter().any(|permission| permission == "*"))
        || (auth["is_super_admin"] != true
            && !(wildcard && permissions.iter().any(|permission| permission == "*"))
            && !required
                .iter()
                .all(|permission| permissions.iter().any(|value| value == permission)))
    {
        return Err("授权权限不满足固定阶段要求".into());
    }
    Ok(json!({
        "subject_id": subject,
        "tenant_id": auth["user"]["tenant_id"],
        "username": auth["user"]["username"],
        "is_super_admin": auth["is_super_admin"],
        "permissions": permissions,
    }))
}

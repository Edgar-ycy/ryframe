//! 原生会话只接受已登记运行的固定调度动作，不开放任意 HTTP 转发。

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::data::performance_identities::producer_authorization;
use crate::local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path};

use super::super::json as json_io;

pub(crate) struct Registration {
    pub root: PathBuf,
    pub run: PathBuf,
    pub attempt: u64,
}

pub(crate) struct Initialization {
    pub artifacts: PathBuf,
    pub identity: Value,
}

impl Registration {
    pub(crate) fn parse(root: &Path, arguments: &[String]) -> Result<Self, &'static str> {
        if arguments.len() != 4 || arguments[0] != "--run-dir" || arguments[2] != "--attempt" {
            return Err("InvalidSessionArguments");
        }
        let attempt = decimal(&arguments[3])
            .filter(|value| *value <= 9_007_199_254_740_991)
            .ok_or("InvalidSessionAttempt")?;
        let run = PathBuf::from(&arguments[1]);
        validate_local_test_path(&run, root, LocalTestPathKind::ExistingDirectory)
            .map_err(|_| "InvalidSessionRun")?;
        Ok(Self {
            root: root.to_path_buf(),
            run,
            attempt,
        })
    }

    pub(crate) fn initialize(&self, request: &Value) -> crate::Result<Initialization> {
        exact(
            request,
            &[
                "id",
                "operation",
                "backend",
                "frontend",
                "config",
                "identity",
                "artifacts",
                "run_dir",
                "attempt",
                "producer_owner",
                "launch",
                "process",
            ],
        )?;
        if request["operation"] != "initialize"
            || request["backend"].as_str() != self.root.to_str()
            || request["run_dir"].as_str() != self.run.to_str()
            || request["attempt"].as_u64() != Some(self.attempt)
        {
            return Err("SessionRegistrationMismatch".into());
        }
        let registration_path = self.run.join("post-copy.json");
        let registration = producer_authorization::binding(&registration_path)?;
        self.authorize(request)?;
        let frontend = Path::new(request["frontend"].as_str().ok_or("InvalidFrontendPath")?);
        validate_absolute_path(frontend, LocalTestPathKind::ExistingDirectory)
            .map_err(|_| "InvalidFrontendPath")?;
        let artifacts = self
            .run
            .join("post-copy")
            .join(format!("attempt-{:04}", self.attempt))
            .join("pacing");
        if request["artifacts"].as_str() != artifacts.to_str() {
            return Err("SessionArtifactsMismatch".into());
        }
        validate_local_test_path(&artifacts, &self.root, LocalTestPathKind::StateDirectory)
            .map_err(|_| "InvalidSessionArtifacts")?;
        identity(&request["identity"])?;
        exact(&request["config"], &["contract", "bindings"])?;
        exact(&request["config"]["contract"], &["timeout_ms", "pacing"])?;
        bindings(&request["config"]["bindings"])?;
        let identity = self.identity(request)?;
        if producer_authorization::binding(&registration_path)? != registration {
            return Err("SessionRegistrationDrift".into());
        }
        Ok(Initialization {
            artifacts,
            identity,
        })
    }

    fn authorize(&self, request: &Value) -> crate::Result<()> {
        let registration = self.run.join("post-copy.json");
        let source = self.root.join("xtask/src/recovery/clone/native/session.rs");
        let launch_path = self
            .run
            .join("post-copy")
            .join(format!("attempt-{:04}", self.attempt))
            .join("session-launch.json");
        let launch = json_io::read(&launch_path)?;
        let tool = launch["tool"].clone();
        let command = vec![
            tool["path"].as_str().ok_or("SessionToolPath")?.to_owned(),
            "__copy-session".into(),
            "--run-dir".into(),
            self.run.to_string_lossy().into_owned(),
            "--attempt".into(),
            self.attempt.to_string(),
        ];
        let state = json_io::read(&self.run.join("state.json"))?;
        let mode = state["attempts"]
            .as_array()
            .and_then(|values| values.last())
            .and_then(|value| value["mode"].as_str())
            .ok_or("SessionAttemptMode")?;
        if !matches!(mode, "schedules" | "reconcile") {
            return Err("SessionAttemptMode".into());
        }
        let spec = producer_authorization::Spec {
            run: &self.run,
            attempt: self.attempt,
            stage: "post-copy",
            mode,
            producer_kind: "session",
            operation: "initialize",
            registration: &registration,
            source: &source,
            command: &command,
            tool: &tool,
            owner_field: "producer_owner",
        };
        let authorization = json!({
            "operation":request["operation"],
            "run_dir":request["run_dir"],
            "attempt":request["attempt"],
            "producer_owner":request["producer_owner"],
            "launch":request["launch"],
            "process":request["process"],
        });
        producer_authorization::authorize(&self.root, &spec, &authorization)?;
        Ok(())
    }

    fn identity(&self, request: &Value) -> crate::Result<Value> {
        let registration = json_io::read(&self.run.join("post-copy.json"))?;
        let source = &registration["source_admin"];
        identity(source)?;
        for field in ["subject_id", "tenant_id", "username", "client_address"] {
            if request["identity"][field] != source[field] {
                return Err("SessionIdentityMismatch".into());
            }
        }
        let password = super::credentials::load(
            &self.root,
            &registration["administrator_credentials"],
            source,
        )?;
        let mut identity = request["identity"].clone();
        identity["password"] = json!(password);
        Ok(identity)
    }
}

pub(crate) fn exact(value: &Value, fields: &[&str]) -> Result<(), &'static str> {
    json_io::exact(value, fields, "原生会话请求")
        .map(|_| ())
        .map_err(|_| "InvalidSessionRequest")
}

pub(crate) fn decimal(value: &str) -> Option<u64> {
    if value.starts_with('0') || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse::<u64>().ok().filter(|value| *value > 0)
}

pub(crate) fn identity(value: &Value) -> Result<(), &'static str> {
    exact(
        value,
        &["subject_id", "tenant_id", "username", "client_address"],
    )?;
    value["subject_id"]
        .as_str()
        .and_then(decimal)
        .ok_or("InvalidSessionIdentity")?;
    if value["tenant_id"] != "system" {
        return Err("InvalidSessionIdentity");
    }
    if value["username"]
        .as_str()
        .is_none_or(|value| value.trim().is_empty())
    {
        return Err("InvalidSessionIdentity");
    }
    let address = value["client_address"]
        .as_str()
        .ok_or("InvalidSessionAddress")?;
    if address
        .strip_prefix("198.19.20.")
        .and_then(|last| {
            last.parse::<u8>()
                .ok()
                .filter(|number| number.to_string() == last)
        })
        .filter(|last| (1..=254).contains(last))
        .is_none()
    {
        return Err("InvalidSessionAddress");
    }
    Ok(())
}

pub(crate) fn bindings(value: &Value) -> Result<(), &'static str> {
    let fields: &[&str] = if value.get("app_config_dir").is_some() {
        &[
            "scope_id",
            "api_url",
            "frontend_url",
            "pacing",
            "app_config_dir",
        ]
    } else {
        &["scope_id", "api_url", "frontend_url", "pacing"]
    };
    exact(value, fields)?;
    if let Some(directory) = value.get("app_config_dir") {
        // 此处保留文本，准备钩子随后按后端根解析相对目录。
        let directory = directory.as_str().ok_or("InvalidAppConfigDirectory")?;
        if directory.is_empty() || directory.chars().any(char::is_control) {
            return Err("InvalidAppConfigDirectory");
        }
    }
    Ok(())
}

pub(crate) fn authorization(response: &Value, identity: &Value) -> Result<Value, &'static str> {
    let auth = &response["data"];
    for (name, field) in [
        ("subject_id", "id"),
        ("tenant_id", "tenant_id"),
        ("username", "username"),
    ] {
        if auth["user"][field] != identity[name] {
            return Err("SessionAuthorizationMismatch");
        }
    }
    let permissions = auth["permissions"]
        .as_array()
        .ok_or("InvalidSessionPermissions")?;
    if !auth["is_super_admin"].is_boolean() || permissions.iter().any(|value| !value.is_string()) {
        return Err("InvalidSessionPermissions");
    }
    let allows = |permission| permissions.iter().any(|value| value == permission);
    if auth["is_super_admin"] != true
        && !allows("*")
        && !(allows("monitor:schedule:list") && allows("monitor:schedule:edit"))
    {
        return Err("SessionPermissionDenied");
    }
    Ok(json!({
        "subject_id": identity["subject_id"], "tenant_id": identity["tenant_id"],
        "username": identity["username"], "is_super_admin": auth["is_super_admin"],
        "permissions": permissions,
    }))
}

pub(crate) fn schedule(request: &Value) -> Result<Value, &'static str> {
    let operation = request["operation"]
        .as_str()
        .ok_or("InvalidSessionOperation")?;
    let fields: &[&str] = match operation {
        "get" => &["id", "operation", "schedule_id"],
        "disable" => &["id", "operation", "schedule_id", "version"],
        _ => return Err("InvalidSessionOperation"),
    };
    exact(request, fields)?;
    request["schedule_id"]
        .as_str()
        .and_then(decimal)
        .ok_or("InvalidScheduleId")?;
    let mut step = json!({
        "operation": if operation == "get" { "get_monitor_schedules_by_id" }
            else { "put_monitor_schedules_by_id_status" },
        "path": {"id": request["schedule_id"]},
    });
    if operation == "disable" {
        request["version"]
            .as_u64()
            .filter(|value| *value <= 9_007_199_254_740_991)
            .ok_or("InvalidScheduleVersion")?;
        step["body"] = json!({"enabled": false, "version": request["version"]});
    }
    Ok(step)
}

//! 生产者只有在 launch、内核身份收据及当前控制器授权全部匹配后才执行。
use super::{evidence, producer_authorization};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::digest,
};
use serde_json::Value;
use std::{
    io::Read,
    path::{Path, PathBuf},
};

pub(super) struct Registration {
    pub(super) run: PathBuf,
    pub(super) attempt: u64,
    pub(super) plan: PathBuf,
    pub(super) state: PathBuf,
    pub(super) mode: String,
    pub(super) plan_sha256: String,
}
impl Registration {
    pub(super) fn parse(arguments: &[String], root: &Path) -> Result<Self> {
        let names = [
            "--run-dir",
            "--attempt",
            "--identity-plan",
            "--identity-state",
            "--identity-mode",
            "--identity-plan-sha256",
        ];
        if arguments.len() != 12 {
            return Err("身份生产者需要固定 run、attempt、计划、账本、模式及文件摘要".into());
        }
        let mut fields = std::collections::BTreeMap::new();
        for pair in arguments.as_chunks::<2>().0 {
            if !names.contains(&pair[0].as_str())
                || pair[1].is_empty()
                || pair[1].starts_with("--")
                || fields.insert(pair[0].as_str(), pair[1].as_str()).is_some()
            {
                return Err("身份生产者参数未知、重复或缺少值".into());
            }
        }
        let get = |key| fields.get(key).copied().ok_or("身份生产者参数缺失");
        let attempt = get("--attempt")?;
        if attempt.starts_with('0') || !attempt.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("身份 attempt 必须为规范正整数".into());
        }
        let attempt = attempt.parse::<u64>()?;
        if !(1..=9_007_199_254_740_991).contains(&attempt) {
            return Err("身份 attempt 超出安全范围".into());
        }
        let mode = get("--identity-mode")?;
        if !matches!(mode, "apply" | "verify") {
            return Err("身份执行模式无效".into());
        }
        let run = PathBuf::from(get("--run-dir")?);
        let plan = PathBuf::from(get("--identity-plan")?);
        let state = PathBuf::from(get("--identity-state")?);
        validate_local_test_path(&run, root, LocalTestPathKind::ExistingDirectory)?;
        validate_local_test_path(&plan, root, LocalTestPathKind::ExistingFile)?;
        validate_local_test_path(&state, root, LocalTestPathKind::StateDirectory)?;
        if state != run.join("seed-runtime/identities") {
            return Err("身份账本必须为当前 run 的固定 seed 目录".into());
        }
        let plan_sha256 = get("--identity-plan-sha256")?.to_owned();
        if plan_sha256.len() != 64
            || !plan_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("身份计划文件 SHA-256 无效".into());
        }
        Ok(Self {
            run,
            attempt,
            plan,
            state,
            mode: mode.into(),
            plan_sha256,
        })
    }
    pub(super) fn authorize(&self, root: &Path, grant: &Value) -> Result<String> {
        if digest::file(&self.plan)?.sha256 != self.plan_sha256 {
            return Err("身份计划原始文件摘要变化".into());
        }
        let original_path = self.run.join("seed-runtime.json");
        let seed =
            crate::recovery::clone::native::seed::Registration::load(root, &self.run, false)?;
        let original_binding = binding(&original_path)?;
        let (registration, selected) =
            crate::recovery::clone::native::seed::identity_successor::select(
                root,
                &self.run,
                self.attempt,
                &seed.value,
                &original_binding,
            )?;
        let registration_path =
            Path::new(selected["path"].as_str().ok_or("身份生产者登记路径缺失")?);
        let tool = &registration["identity_tool"];
        let command = self.command(tool)?;
        let mode = format!("identities-{}", self.mode);
        let source = root.join("xtask/src/data/performance_identities/producer.rs");
        let producer_kind = format!("identity-{}", self.mode);
        let spec = producer_authorization::Spec {
            run: &self.run,
            attempt: self.attempt,
            stage: "seed-runtime",
            mode: &mode,
            producer_kind: &producer_kind,
            operation: "start",
            registration: registration_path,
            source: &source,
            command: &command,
            tool,
            owner_field: "identity_lock_owner",
        };
        let token = producer_authorization::authorize(root, &spec, grant)?;
        if registration["identity_plan"] != binding(&self.plan)?
            || registration["identity_state"].as_str() != self.state.to_str()
        {
            return Err("身份生产者登记未绑定固定计划或账本".into());
        }
        if digest::file(&self.plan)?.sha256 != self.plan_sha256 {
            return Err("身份生产者授权期间计划变化".into());
        }
        if binding(registration_path)? != selected {
            return Err("身份生产者的有效登记在授权期间变化".into());
        }
        seed.assert_unchanged(root, false)?;
        Ok(token)
    }

    fn command(&self, tool: &Value) -> Result<Vec<String>> {
        let mut command = vec![
            tool["path"].as_str().ok_or("身份工具路径缺失")?.to_owned(),
            "__identity-producer".into(),
        ];
        for (key, value) in [
            ("--run-dir", self.run.to_string_lossy().into_owned()),
            ("--attempt", self.attempt.to_string()),
            ("--identity-plan", self.plan.to_string_lossy().into_owned()),
            (
                "--identity-state",
                self.state.to_string_lossy().into_owned(),
            ),
            ("--identity-mode", self.mode.clone()),
            ("--identity-plan-sha256", self.plan_sha256.clone()),
        ] {
            command.push(key.into());
            command.push(value);
        }
        Ok(command)
    }
}
pub(super) fn binding(path: &Path) -> Result<Value> {
    producer_authorization::binding(path)
}
pub(super) fn run_hidden_if_requested() -> Option<i32> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.first().map(String::as_str) != Some("__identity-producer") {
        return None;
    }
    let result = (|| {
        let root = crate::workspace::root_dir();
        let registration = Registration::parse(&arguments[1..], &root)?;
        let mut bytes = Vec::new();
        std::io::stdin().take(32769).read_to_end(&mut bytes)?;
        if bytes.len() > 32768 {
            return Err("身份生产者启动授权超过 32 KiB".into());
        }
        let token = registration.authorize(&root, &evidence::parse(&bytes)?)?;
        let current =
            crate::recovery::clone::native::seed::Current::load(&root, &registration.run)?;
        super::execute_at(
            &registration.plan,
            &registration.state,
            &registration.mode,
            &token,
            Some(&registration.plan_sha256),
            &mut |_| current.check(&root),
        )
    })();
    Some(match result {
        Ok(value) => {
            println!("{}", value);
            0
        }
        Err(_) => {
            eprintln!(
                "identity_producer_failed：身份生产者未完成；核对原阶段、进程收据及账本，禁止自动重放。"
            );
            1
        }
    })
}

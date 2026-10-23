//! 生产者只有在 launch、内核身份收据及当前控制器授权全部匹配后才执行。
use super::{evidence, run_lock};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{digest, json as document, lifecycle::identity},
};
use serde_json::{Value, json};
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
        document::exact(
            grant,
            &[
                "operation",
                "run_dir",
                "attempt",
                "identity_lock_owner",
                "launch",
                "process",
            ],
            "身份启动授权",
        )?;
        let token = grant["identity_lock_owner"]
            .as_str()
            .ok_or("身份授权缺少锁 owner")?;
        if grant["operation"] != "start"
            || grant["run_dir"].as_str() != self.run.to_str()
            || grant["attempt"] != self.attempt
            || !run_lock::uuid(token)
        {
            return Err("身份启动授权不属于当前 run/attempt/owner".into());
        }
        if digest::file(&self.plan)?.sha256 != self.plan_sha256 {
            return Err("身份计划原始文件摘要变化".into());
        }
        let output = self
            .run
            .join("seed-runtime")
            .join(format!("attempt-{:04}", self.attempt));
        let launch_path = output.join("session-launch.json");
        let process_path = output.join("session-process.json");
        if grant["launch"] != binding(&launch_path)? || grant["process"] != binding(&process_path)?
        {
            return Err("身份授权未绑定已持久化 launch 与内核收据".into());
        }
        let launch = document::read(&launch_path)?;
        let process = document::read(&process_path)?;
        document::exact(
            &launch,
            &[
                "format_version",
                "kind",
                "run_manifest",
                "registration",
                "controller",
                "attempt",
                "producer_kind",
                "command",
                "tool",
                "source",
            ],
            "原生身份 launch",
        )?;
        document::exact(
            &process,
            &[
                "format_version",
                "kind",
                "launch",
                "identity",
                "identity_lock_owner",
            ],
            "原生身份进程",
        )?;
        if process["identity"] != identity::current()?
            || process["launch"] != grant["launch"]
            || process["identity_lock_owner"] != token
            || process["format_version"] != 1
            || process["kind"] != "devex-post-producer"
        {
            return Err("原生身份生产者内核身份或锁 owner 不符".into());
        }
        self.launch(root, &launch)?;
        if binding(&launch_path)? != grant["launch"] || binding(&process_path)? != grant["process"]
        {
            return Err("身份生产者授权核验期间证据变化".into());
        }
        Ok(token.into())
    }
    fn launch(&self, root: &Path, launch: &Value) -> Result<()> {
        let registration_path = self.run.join("seed-runtime.json");
        let registration = document::read(&registration_path)?;
        let controller_path = self
            .run
            .join(format!("controller-{:04}.json", self.attempt));
        let controller = document::read(&controller_path)?;
        let state = document::read(&self.run.join("state.json"))?;
        let attempt = state["attempts"]
            .as_array()
            .and_then(|attempts| attempts.last())
            .ok_or("身份 run 缺少当前 attempt")?;
        let mode = format!("identities-{}", self.mode);
        if attempt["number"] != self.attempt
            || attempt["stage"] != "seed-runtime"
            || attempt["mode"] != mode
            || attempt["status"] != "running"
            || controller["attempt_sha256"] != digest::plan_hash(attempt)?
            || controller["attempt"] != self.attempt
            || controller["kind"] != "devex-stage-controller"
            || controller["format_version"] != 1
            || controller["owner"] != document::read(&self.run.join("run.lock/owner.json"))?
            || controller["owner"]["directory"].as_str() != self.run.to_str()
            || controller["owner"]["manifest_sha256"]
                != binding(&self.run.join("manifest.json"))?["sha256"]
        {
            return Err("身份生产者控制器不持当前 run 锁或不属于当前 attempt".into());
        }
        let controller_id = &controller["owner"]["identity"];
        let pid = controller_id["pid"]
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
            .ok_or("身份控制器 PID 无效")?;
        if identity::process(pid)?.as_ref() != Some(controller_id) {
            return Err("身份控制器已退出或 PID 代次变化".into());
        }
        let executable = std::env::current_exe()?;
        let tool = &launch["tool"];
        document::exact(tool, &["path", "sha256"], "原生身份工具")?;
        let recorded = Path::new(tool["path"].as_str().ok_or("原生身份工具路径缺失")?);
        if !same_file::is_same_file(recorded, &executable)?
            || tool["sha256"] != digest::file(&executable)?.sha256
            || *tool != registration["identity_tool"]
        {
            return Err("原生身份工具身份或字节摘要与登记不同".into());
        }
        let source = root.join("xtask/src/data/performance_identities/producer.rs");
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
        if launch["format_version"] != 1
            || launch["kind"] != "devex-post-producer-launch"
            || launch["producer_kind"] != format!("identity-{}", self.mode)
            || launch["attempt"] != self.attempt
            || launch["run_manifest"] != binding(&self.run.join("manifest.json"))?
            || launch["registration"] != binding(&registration_path)?
            || launch["controller"] != binding(&controller_path)?
            || launch["source"] != binding(&source)?
            || launch["command"] != json!(command)
            || registration["identity_plan"] != binding(&self.plan)?
            || registration["identity_state"].as_str() != self.state.to_str()
        {
            return Err("原生身份 launch 未绑定固定命令、来源、计划或登记".into());
        }
        Ok(())
    }
}
pub(super) fn binding(path: &Path) -> Result<Value> {
    // Python Path 在 Windows 使用统一分隔符；登记与执行不能因 join 中的斜杠不同而失配。
    let path = path.components().collect::<PathBuf>();
    let digest = digest::file(&path)?;
    Ok(json!({"path":path,"bytes":digest.bytes,"sha256":digest.sha256}))
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
        super::execute_at(
            &registration.plan,
            &registration.state,
            &registration.mode,
            &token,
            Some(&registration.plan_sha256),
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

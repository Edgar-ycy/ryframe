//! seed 的有界文件证据协议；不启动服务，不接收凭据或替代运行时守卫。
use super::{Registration, history, identity, runtime_model};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{
        digest, json as document,
        lifecycle::{identity as process, state},
        resources::common,
    },
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::Path,
};

#[path = "guards/mod.rs"]
pub(crate) mod guards;

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().map(String::as_str) == Some("__seed-runtime-control") {
        return Some(super::pair_entry::run_hidden(&args));
    }
    if args.first().map(String::as_str) != Some("__seed-evidence") {
        return None;
    }
    let result = (|| {
        if args.len() != 1 {
            return Err("seed 证据协议只接受有界标准输入".into());
        }
        let mut bytes = Vec::new();
        std::io::stdin().take(65537).read_to_end(&mut bytes)?;
        execute(&crate::workspace::root_dir(), &bytes)
    })();
    // 配置解析的底层错误可能包含私有 TOML 行；协议只发布固定分类。
    let (code, response) = match result {
        Ok(value) => (0, json!({"ok":true,"result":value})),
        Err(_) => (
            1,
            json!({"ok":false,"error":{"category":"SeedEvidenceRejected",
            "message":"seed 文件证据、当前控制器或代次无效；保留原始文件、阶段结果与锁，核对绑定配置和来源后显式恢复，禁止自动重放。",
            "automatic_retry_permitted":false}}),
        ),
    };
    Some(
        if writeln!(std::io::stdout().lock(), "{response}").is_ok() {
            code
        } else {
            1
        },
    )
}

pub(crate) fn execute(backend: &Path, bytes: &[u8]) -> Result<Value> {
    if bytes.is_empty() || bytes.len() > 65536 {
        return Err("seed 证据请求为空或超过64KiB".into());
    }
    let request = document::parse_fragment(std::str::from_utf8(bytes)?, "seed 证据协议")?;
    document::exact(
        &request,
        &[
            "format_version",
            "kind",
            "run_dir",
            "operation",
            "arguments",
        ],
        "seed 证据协议",
    )?;
    if request["format_version"].as_u64() != Some(1) || request["kind"] != "ryframe-seed-evidence" {
        return Err("seed 证据协议版本或类型无效".into());
    }
    let run = Path::new(document::text(&request["run_dir"], "seed run")?);
    validate_local_test_path(run, backend, LocalTestPathKind::ExistingDirectory)?;
    let args = &request["arguments"];
    match request["operation"].as_str() {
        Some("process-time") => {
            document::exact(args, &["pid"], "本机进程创建时间参数")?;
            let pid = args["pid"]
                .as_u64()
                .and_then(|pid| u32::try_from(pid).ok())
                .filter(|pid| *pid > 1)
                .ok_or("本机进程查询必须指定大于 1 的 u32 PID")?;
            super::source_registration::quiet::cim::query(pid)
        }
        Some("close-rebind-guard") => guards::dispatch(backend, run, args),
        Some("generation-guard") => super::generation::dispatch(backend, run, args),
        Some("department-guard") => super::super::department::dispatch(backend, run, args),
        Some("register") => {
            document::exact(
                args,
                &["request", "attempt", "controller", "target_lock_inode"],
                "seed 登记参数",
            )?;
            let guard = Controller::load(backend, run, args)?;
            let path = Path::new(document::text(&args["request"], "seed 请求")?);
            let value = super::publish(backend, run, path)?;
            guard.check(backend, run, args)?;
            Ok(value)
        }
        Some("registration") => {
            document::exact(args, &["cleanup"], "seed 登记读取参数")?;
            let cleanup = args["cleanup"].as_bool().ok_or("cleanup 必须为布尔值")?;
            Ok(Registration::load(backend, run, cleanup)?.value)
        }
        Some("inputs") => {
            document::exact(args, &[], "seed 当前输入参数")?;
            let proof = Registration::load(backend, run, false)?;
            let post = history::validate(backend, run, &proof.value)?;
            let plan = identity::inputs(backend, run, &proof.value, &post)?;
            proof.assert_unchanged(backend, false)?;
            Ok(json!({"post":post.request,"identity_plan":plan}))
        }
        Some("identity-evidence") => {
            document::exact(args, &["request"], "seed 身份发布参数")?;
            let proof = Registration::load(backend, run, false)?;
            if proof.value != args["request"] {
                return Err("seed 身份发布请求不同于当前已登记请求".into());
            }
            let plan = common::read_bound(backend, &proof.value["identity_plan"], "seed 身份计划")?;
            let root = Path::new(document::text(
                &proof.value["identity_state"],
                "seed 身份账本",
            )?);
            let result =
                crate::data::performance_identities::seed_evidence::verify(backend, &plan, root)?;
            proof.assert_unchanged(backend, false)?;
            Ok(result)
        }
        Some("handoff" | "stopped" | "restart") => {
            runtime(backend, run, request["operation"].as_str().unwrap(), args)
        }
        _ => Err("未知 seed 证据操作".into()),
    }
}

fn runtime(backend: &Path, run: &Path, operation: &str, args: &Value) -> Result<Value> {
    let inputs = runtime_model::inputs(backend, run)?;
    match operation {
        "handoff" => {
            document::exact(args, &[], "seed 清理交接参数")?;
            Ok(
                json!({"runtime":inputs.runtime,"handoff":inputs.handoff,"environment":inputs.environment}),
            )
        }
        "stopped" => {
            document::exact(args, &[], "seed 停止证据参数")?;
            runtime_model::stopped(&inputs.runtime, &inputs.handoff)
        }
        "restart" => {
            document::exact(args, &["failed", "current"], "seed 重启守卫参数")?;
            let current = args["current"]
                .as_u64()
                .filter(|value| *value > 0)
                .ok_or("当前阶段序号无效")?;
            runtime_model::restart_after_stop(
                backend,
                run,
                &inputs.runtime,
                &inputs.handoff,
                &args["failed"],
                current,
            )?;
            Ok(json!({"restart_ready":true}))
        }
        _ => Err("未知 seed 运行证据操作".into()),
    }
}

struct Controller {
    state: Value,
    controller: Value,
    owner: Value,
    target: Value,
}
impl Controller {
    fn load(backend: &Path, run: &Path, args: &Value) -> Result<Self> {
        let saved = state::load(run, true)?;
        let attempt = state::running_attempt(&saved)?.ok_or("seed 登记缺少当前持锁阶段")?;
        let number = args["attempt"]
            .as_u64()
            .filter(|v| *v > 0)
            .ok_or("seed 登记序号无效")?;
        let descriptor = common::binding(&state::controller_path(run, number))?;
        let controller = common::read_bound(backend, &descriptor, "seed 控制器")?;
        let owner = document::read(&run.join("run.lock/owner.json"))?;
        process::validate(&owner, run)?;
        let pid = owner["identity"]["pid"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or("控制器 PID 无效")?;
        if attempt["number"] != number
            || attempt["stage"] != "seed-runtime"
            || attempt["mode"] != "register"
            || args["controller"] != descriptor
            || controller["format_version"].as_u64() != Some(1)
            || controller["kind"] != "devex-stage-controller"
            || controller["attempt"] != number
            || controller["attempt_sha256"].as_str() != Some(digest::plan_hash(attempt)?.as_str())
            || controller["owner"] != owner
            || owner["manifest_sha256"] != state::manifest_binding(run)?["sha256"]
            || process::process(pid)?.as_ref() != Some(&owner["identity"])
        {
            return Err("seed 登记不属于当前存活控制器及运行代次".into());
        }
        let manifest = document::read(&run.join("manifest.json"))?;
        let initialized = common::bound_file(backend, &manifest["initialized"], "seed 初始化来源")?;
        let target = initialized
            .parent()
            .ok_or("初始化来源无父目录")?
            .join("initialize.lock");
        validate_local_test_path(&target, backend, LocalTestPathKind::ExistingDirectory)?;
        let (_, inode) = common::device_inode_for(&target, &std::fs::symlink_metadata(&target)?)?;
        if args["target_lock_inode"].as_u64() != Some(inode) {
            return Err("seed 目标锁已被替换".into());
        }
        Ok(Self {
            state: saved,
            controller: descriptor,
            owner,
            target: json!({"path":target,"inode":inode}),
        })
    }
    fn check(&self, backend: &Path, run: &Path, args: &Value) -> Result<()> {
        let after = Self::load(backend, run, args)?;
        if self.state != after.state
            || self.controller != after.controller
            || self.owner != after.owner
            || self.target != after.target
        {
            return Err("seed 登记发布期间控制器、阶段或目标锁变化；保留现场".into());
        }
        Ok(())
    }
}

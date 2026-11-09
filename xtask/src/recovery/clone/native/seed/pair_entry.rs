//! 既有 seed 编排的受管角色控制。只有实际持锁父控制器能调用。

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{runtime_model, runtime_pair, start_authority};
use crate::{
    Result,
    recovery::{
        clone::native::{
            cache::handoff::locks::seed_runtime_guard,
            json as document,
            lifecycle::{identity, lock::ProcessGuard, state},
            post_process::runtime,
            resources::common,
            target_verify::guard::Inputs,
        },
        full_stack::worker::{controller, role::Role},
    },
};

#[path = "pair_entry/drain.rs"]
mod drain;
#[path = "pair_entry/drain_target.rs"]
mod drain_target;
#[path = "pair_entry/receipts.rs"]
mod receipts;
#[path = "pair_entry/startup.rs"]
mod startup;
pub(in crate::recovery::clone::native) use startup::run as start;

const PAIR: &[Role] = &[Role::Api, Role::Worker];

#[path = "pair_entry/close.rs"]
mod close;
pub(in crate::recovery::clone::native) use close::run as close_runtime;

pub(super) fn run_hidden(args: &[String]) -> i32 {
    let result = (|| {
        if args.len() != 1 {
            return Err("seed 角色控制只接受有界标准输入".into());
        }
        let mut bytes = Vec::new();
        std::io::stdin().take(65537).read_to_end(&mut bytes)?;
        execute(&crate::workspace::root_dir(), &bytes)
    })();
    let (code, value) = match result {
        Ok(result) => (0, json!({"ok":true,"result":result})),
        Err(_) => (
            1,
            json!({"ok":false,"error":{"category":"SeedRuntimeControlUnresolved",
            "message":"原 seed 角色控制未取得完整结果；核对固定 attempt、监督树和角色收据，禁止自动重放。",
            "automatic_retry_permitted":false}}),
        ),
    };
    if writeln!(std::io::stdout().lock(), "{value}").is_ok() {
        code
    } else {
        1
    }
}

fn execute(backend: &Path, bytes: &[u8]) -> Result<Value> {
    if bytes.is_empty() || bytes.len() > 65536 {
        return Err("seed 角色控制请求为空或超过 64 KiB".into());
    }
    let request = document::parse_fragment(std::str::from_utf8(bytes)?, "seed 角色控制请求")?;
    let operation = document::text(&request["operation"], "seed 角色操作")?;
    let mut fields = vec![
        "format_version",
        "kind",
        "run_dir",
        "attempt",
        "operation",
        "roles",
        "handoff",
    ];
    if operation == "drain" {
        fields.push("target_lock_inode");
    }
    document::exact(&request, &fields, "seed 角色控制请求")?;
    if request["format_version"] != 1 || request["kind"] != "ryframe-seed-runtime-control" {
        return Err("seed 角色控制版本或类型无效".into());
    }
    let run = common::local_directory(
        backend,
        Path::new(document::text(&request["run_dir"], "seed 运行目录")?),
        "seed 固定运行目录",
    )?;
    let roles = document::array(&request["roles"], "seed 角色")?
        .iter()
        .map(|role| match role.as_str() {
            Some("api") => Ok(Role::Api),
            Some("worker") => Ok(Role::Worker),
            _ => Err("seed 只控制 API 和 Worker"),
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if matches!(operation, "start" | "recover") || operation == "stop" && roles.as_slice() == PAIR {
        return Err("显式 seed 启动、双停和回收只使用 Rust 正式阶段".into());
    }
    let number = request["attempt"].as_u64().ok_or("seed 控制阶段序号无效")?;
    let frame = Frame::load(
        backend,
        &run,
        operation,
        &roles,
        number,
        parent_process()?,
        Some(&request["handoff"]),
    )?;
    frame.check()?;
    if operation == "drain" {
        let inode = request["target_lock_inode"]
            .as_u64()
            .ok_or("seed 排空缺少目标锁身份")?;
        return Ok(json!({"snapshot":drain_target::observe(&frame,inode)?}));
    }
    control(&frame, operation, &roles)
}

fn control(frame: &Frame<'_>, operation: &str, roles: &[Role]) -> Result<Value> {
    if operation == "recover" {
        return Ok(json!({"runtime":receipts::recover(frame,roles)?,"authority":null}));
    }
    let authority = if operation == "start" {
        start_authority::verify(frame.backend, frame.run, frame.number)?
    } else {
        Value::Null
    };
    let close = frame.close_stop(operation, roles)?;
    let protected = Inputs::open(
        close
            .iter()
            .map(|descriptor| {
                common::bound_file(frame.backend, descriptor, "关闭前已验证的 checkpoint")
            })
            .collect::<Result<Vec<_>>>()?,
    )?;
    if matches!(operation, "start" | "stop") {
        receipts::claim(frame, operation, roles)?;
    }
    let checked = || {
        frame.check()?;
        protected.check()?;
        for descriptor in &close {
            common::bound_file(frame.backend, descriptor, "关闭前已验证的 checkpoint")?;
        }
        Ok(())
    };
    let value = runtime_pair::control(
        frame.inputs()?,
        operation,
        roles,
        &checked,
        &|role, id| receipts::reserved(frame, role, id),
        &|kind, role, token, identity, id| {
            receipts::reserve_stop(frame, roles, kind, role, token, identity, id)
        },
    )?;
    checked()?;
    Ok(json!({"runtime":value,"authority":authority}))
}

pub(super) fn cleanup(backend: &Path, run: &Path, number: u64, operation: &str) -> Result<Value> {
    if !matches!(operation, "stop" | "recover") {
        return Err("seed 清理只接受明确的停止或收据回收".into());
    }
    let frame = Frame::load(
        backend,
        run,
        operation,
        PAIR,
        number,
        std::process::id(),
        None,
    )?;
    let mut result = control(&frame, operation, PAIR)?;
    let runtime = result
        .as_object_mut()
        .and_then(|value| value.remove("runtime"))
        .ok_or("seed 角色清理缺少结果")?;
    let observed = if operation == "stop" {
        stop_result(
            runtime,
            &frame.handoff,
            runtime_model::stopped(&frame.runtime.runtime, &frame.runtime.handoff),
        )
    } else {
        json!({"status":"seed_runtime_recover","runtime":runtime,"restore_qualified":false})
    };
    frame.check()?;
    Ok(observed)
}

fn stop_result(runtime: Value, handoff: &Value, evidence: Result<Value>) -> Value {
    let mut result = json!({"status":"seed_runtime_stop","runtime":runtime,
        "handoff":handoff,"restore_qualified":false,"restart_ready":evidence.is_ok()});
    match evidence {
        Ok(evidence) => result["stopped_evidence"] = evidence,
        Err(error) => result["stopped_evidence_error_type"] = json!(stop_error(error.as_ref())),
    }
    result
}

fn stop_error(error: &(dyn std::error::Error + 'static)) -> &'static str {
    use std::io::ErrorKind;
    match error
        .downcast_ref::<std::io::Error>()
        .map(std::io::Error::kind)
    {
        Some(ErrorKind::NotFound) => "FileNotFoundError",
        Some(ErrorKind::PermissionDenied) => "PermissionError",
        Some(_) => "OSError",
        None => "ValueError",
    }
}

pub(super) fn status(backend: &Path, run: &Path) -> Result<Value> {
    let saved = state::load(run, false)?;
    let inputs = runtime_model::inputs(backend, run)?;
    let handoff = common::binding(&inputs.runtime.join("handoff.json"))?;
    let contract = common::read_bound(backend, &inputs.handoff["runtime"], "seed 状态合同")?;
    let product = runtime::product_root(backend, &contract)?;
    let check = || {
        if state::load(run, false)? != saved
            || common::binding(&inputs.runtime.join("handoff.json"))? != handoff
            || runtime::contract(backend, &product, &inputs.runtime)? != contract
        {
            return Err("seed 状态观察期间原运行交接变化".into());
        }
        Ok(())
    };
    check()?;
    let value = runtime_pair::control(
        runtime_pair::Inputs {
            backend,
            product: &product,
            runtime: &inputs.runtime,
            contract: &contract,
            environment: &inputs.handoff["api_environment"],
            api_url: document::text(&inputs.handoff["api_url"], "seed API 地址")?,
        },
        "status",
        PAIR,
        &check,
        &|_, _| Err("只读观察不能发布启动预留".into()),
        &|_, _, _, _, _| Err("只读观察不能发布停止预留".into()),
    )?;
    check()?;
    status_result(value)
}

fn status_result(mut value: Value) -> Result<Value> {
    value["kind"] = json!("devex-clone-runtime-observation");
    value
        .as_object_mut()
        .ok_or("seed 状态无效")?
        .remove("producer_history");
    Ok(json!({"status":"seed_runtime_status","runtime":value,"restore_qualified":false}))
}

struct Frame<'a> {
    backend: &'a Path,
    run: &'a Path,
    number: u64,
    saved: Value,
    owner: Value,
    owner_pid: u32,
    controller: Value,
    executor: Value,
    runtime: runtime_model::Inputs,
    handoff: Value,
    product: PathBuf,
    contract: Value,
    guard: ProcessGuard,
    protected: Inputs,
}

impl<'a> Frame<'a> {
    fn load(
        backend: &'a Path,
        run: &'a Path,
        operation: &str,
        roles: &[Role],
        number: u64,
        owner_pid: u32,
        expected_handoff: Option<&Value>,
    ) -> Result<Self> {
        let saved = state::load(run, false)?;
        let attempt = state::running_attempt(&saved)?.ok_or("seed 控制缺少当前执行阶段")?;
        if attempt["stage"] != "seed-runtime" || attempt["number"].as_u64() != Some(number) {
            return Err("seed 控制请求不属于当前执行阶段".into());
        }
        allowed(
            document::text(&attempt["mode"], "seed 阶段")?,
            operation,
            roles,
        )?;
        let (controller, owner) = state::controller_record(run, number, attempt)?;
        identity::validate(&owner, run)?;
        let executor = identity::current()?;
        if owner["identity"]["pid"].as_u64() != Some(u64::from(owner_pid))
            || owner["manifest_sha256"] != state::manifest_binding(run)?["sha256"]
            || identity::process(owner_pid)?.as_ref() != Some(&owner["identity"])
            || document::read(&run.join("run.lock/owner.json"))? != owner
        {
            return Err("seed 角色控制不属于实际当前 run owner".into());
        }
        let runtime = runtime_model::inputs(backend, run)?;
        let handoff = common::binding(&runtime.runtime.join("handoff.json"))?;
        if expected_handoff.is_some_and(|expected| *expected != handoff) {
            return Err("seed 角色控制与已发布运行交接不同".into());
        }
        let contract =
            common::read_bound(backend, &runtime.handoff["runtime"], "seed 固定角色合同")?;
        let product = runtime::product_root(backend, &contract)?;
        let guard = seed_runtime_guard(backend, run)?;
        let mut paths = vec![
            run.join("state.json"),
            run.join("run.lock/owner.json"),
            run.join("manifest.json"),
            common::bound_file(backend, &controller, "seed 原阶段控制器")?,
        ];
        for binding in [
            &handoff,
            &runtime.handoff["registration"],
            &runtime.handoff["runtime"],
            &runtime.handoff["binaries"],
            &runtime.handoff["api_environment"],
        ] {
            paths.push(common::bound_file(backend, binding, "seed 固定角色输入")?);
        }
        let protected = Inputs::open(paths)?;
        let frame = Self {
            backend,
            run,
            number,
            saved,
            owner,
            owner_pid,
            controller,
            executor,
            runtime,
            handoff,
            product,
            contract,
            guard,
            protected,
        };
        frame.check()?;
        Ok(frame)
    }

    fn check(&self) -> Result<()> {
        self.guard
            .check_named(&self.run.join("seed-runtime"), "runtime-control.guard")?;
        self.protected.check()?;
        if state::load(self.run, false)? != self.saved
            || document::read(&self.run.join("run.lock/owner.json"))? != self.owner
            || identity::process(self.owner_pid)?.as_ref() != Some(&self.owner["identity"])
            || common::binding(&self.runtime.runtime.join("handoff.json"))? != self.handoff
            || runtime::contract(self.backend, &self.product, &self.runtime.runtime)?
                != self.contract
        {
            return Err("seed 角色控制期间控制器、状态或冻结合同变化".into());
        }
        self.protected.check()
    }

    fn inputs(&self) -> Result<runtime_pair::Inputs<'_>> {
        Ok(runtime_pair::Inputs {
            backend: self.backend,
            product: &self.product,
            runtime: &self.runtime.runtime,
            contract: &self.contract,
            environment: &self.runtime.handoff["api_environment"],
            api_url: document::text(&self.runtime.handoff["api_url"], "seed API 地址")?,
        })
    }

    fn close_stop(&self, operation: &str, roles: &[Role]) -> Result<Vec<Value>> {
        let attempt = state::running_attempt(&self.saved)?.ok_or("seed close 缺少当前阶段")?;
        if attempt["mode"] != "close" || operation != "stop" {
            return Ok(Vec::new());
        }
        let target = if roles == [Role::Api] { 0 } else { 2 };
        let (result, descriptors) = close::authorize_stop(self, target)?;
        for role in [Role::Api, Role::Worker] {
            let observed = controller::observe_bound_state(
                role,
                &self.product,
                &self.runtime.runtime,
                &self.runtime.handoff["api_environment"],
                false,
            )?;
            if target == 2 && role == Role::Api {
                if observed["state"] != "stopped" || !observed["identity"].is_null() {
                    return Err("关闭 Worker 前 API 尚未完整停止".into());
                }
            } else if observed["state"] != "running"
                || observed["identity"] != result["process_identities"][role.name()]
            {
                return Err("seed close 当前角色创建身份不同于检查点".into());
            }
        }
        Ok(descriptors)
    }
}

fn allowed(mode: &str, operation: &str, roles: &[Role]) -> Result<()> {
    let pair = roles == [Role::Api, Role::Worker];
    let valid = match (mode, operation) {
        ("start", "start") | ("stop", "stop") | ("recover", "recover") | ("close", "status") => {
            pair
        }
        ("close", "stop") => matches!(roles, [Role::Api] | [Role::Worker]),
        ("close", "drain") => roles == [Role::Worker],
        _ => false,
    };
    if !valid {
        return Err("seed 角色操作不同于当前阶段的固定角色集合".into());
    }
    Ok(())
}

#[cfg(windows)]
fn parent_process() -> Result<u32> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Process {
        parent_process_id: u32,
    }
    let query = format!(
        "SELECT ParentProcessId FROM Win32_Process WHERE ProcessId={}",
        std::process::id()
    );
    let connection = wmi::WMIConnection::new().map_err(|_| "无法观察 seed 控制器父进程")?;
    let rows: Vec<Process> = connection
        .raw_query(&query)
        .map_err(|_| "无法查询 seed 控制器父进程")?;
    let [current] = rows.as_slice() else {
        return Err("seed 控制器父进程观察不唯一".into());
    };
    Ok(current.parent_process_id)
}

#[cfg(unix)]
fn parent_process() -> Result<u32> {
    Ok(u32::try_from(nix::unistd::getppid().as_raw())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_and_control_are_scoped_to_the_current_seed_stage() {
        let pair = [Role::Api, Role::Worker];
        assert!(allowed("start", "start", &pair).is_ok());
        assert!(allowed("close", "status", &pair).is_ok());
        assert!(allowed("close", "stop", &[Role::Worker]).is_ok());
        assert!(allowed("close", "drain", &[Role::Worker]).is_ok());
        assert!(allowed("close", "drain", &pair).is_err());
        assert!(allowed("status", "drain", &[Role::Worker]).is_err());
        assert!(allowed("close", "stop", &pair).is_err());
        assert!(allowed("status", "stop", &pair).is_err());
        assert!(allowed("prepare", "start", &pair).is_err());
        assert!(allowed("recover", "recover", &[Role::Worker]).is_err());
    }

    #[test]
    fn public_status_uses_observation_contract_without_producer_history() {
        let result =
            status_result(json!({"operation":"status","producer_history":{"private":true}}))
                .unwrap();
        assert_eq!(
            result,
            json!({"status":"seed_runtime_status","restore_qualified":false,
            "runtime":{"operation":"status","kind":"devex-clone-runtime-observation"}})
        );
    }

    #[test]
    fn stopping_can_succeed_without_proving_a_restart_or_business_drain() {
        let runtime = json!({"operation":"stop","processes":{}});
        let handoff = json!({"sha256":"fixture"});
        let evidence = json!({"history":{"fixture":true},"processes":{}});
        let stopped = stop_result(runtime.clone(), &handoff, Ok(evidence.clone()));
        assert_eq!(
            stopped,
            json!({"status":"seed_runtime_stop","runtime":runtime,"handoff":handoff,
            "restore_qualified":false,"restart_ready":true,"stopped_evidence":evidence})
        );
        let incomplete = stop_result(runtime.clone(), &handoff, Err("缺少启动收尾".into()));
        assert_eq!(
            incomplete,
            json!({"status":"seed_runtime_stop","runtime":runtime,"handoff":handoff,
            "restore_qualified":false,"restart_ready":false,"stopped_evidence_error_type":"ValueError"})
        );
    }
}

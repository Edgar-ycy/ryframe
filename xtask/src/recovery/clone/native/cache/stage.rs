//! 缓存固定运行、登记与控制器边界。
use super::rejected;
pub(super) mod execute;
pub(super) mod successor;
use super::super::{
    json as doc,
    lifecycle::{identity, state},
    resources::common,
};
use super::{history, session::Runtime};
use crate::Result;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) const STAGE: &str = "cache-target";
pub(super) struct Context<'a> {
    pub backend: &'a Path,
    pub directory: PathBuf,
    pub caller: u32,
    pub runtime: &'a mut dyn Runtime,
}
impl Context<'_> {
    pub fn call(&mut self, name: &str, args: Value) -> Result<Value> {
        self.runtime.call(name, args)
    }
    pub fn records(&self, cleanup: bool) -> Result<Vec<Value>> {
        Ok(state::attempts(&state::load(&self.directory, !cleanup)?)?
            .iter()
            .filter(|r| r["stage"] == STAGE)
            .cloned()
            .collect())
    }
    pub fn output(&self, number: u64) -> PathBuf {
        self.directory.join(STAGE).join(format!("a{number:04}"))
    }
    pub fn manifest(&self) -> Result<Value> {
        doc::read(&self.directory.join("manifest.json"))
    }
    pub fn controller(&self, attempt: &Value) -> Result<Value> {
        let mut initial = attempt.clone();
        initial["status"] = json!("running");
        initial["finished_at"] = Value::Null;
        initial["result"] = Value::Null;
        initial["error_type"] = Value::Null;
        let (binding, owner) =
            state::controller_record(&self.directory, number(attempt)?, &initial)?;
        if path(&owner["directory"])? != self.directory
            || owner["manifest_sha256"]
                != common::binding(&self.directory.join("manifest.json"))?["sha256"]
        {
            return Err(rejected("缓存控制器不属于固定运行和完整阶段"));
        }
        Ok(binding)
    }
    pub fn active(&self, mode: &str, number: u64) -> Result<Value> {
        let state = state::load(&self.directory, !matches!(mode, "stop" | "recover"))?;
        let attempt = state::attempts(&state)?
            .last()
            .ok_or("缓存操作没有当前阶段")?;
        if attempt["number"].as_u64() != Some(number)
            || attempt["stage"] != STAGE
            || attempt["mode"] != mode
            || attempt["status"] != "running"
        {
            return Err(rejected("缓存操作不是统一运行中当前唯一已登记阶段"));
        }
        let descriptor = self.controller(attempt)?;
        let controller = common::read_bound(self.backend, &descriptor, "缓存控制器")?;
        if Some(controller["owner"]["identity"].clone()) != identity::process(self.caller)?
            || controller["owner"] != doc::read(&self.directory.join("run.lock/owner.json"))?
        {
            return Err(rejected("缓存操作必须由原控制器持有统一运行锁"));
        }
        Ok(descriptor)
    }
    pub fn registration(&self) -> Result<(Value, Value)> {
        registration(self.backend, &self.directory)
    }
    pub fn validate(&mut self, request: &Value, lock: &Value) -> Result<Value> {
        self.call(
            "validate",
            json!({"request":request,"owned_lock_identity":lock}),
        )
    }
    pub fn register(&mut self, filename: Option<&Path>) -> Result<(Value, Value)> {
        let root = self.directory.join(STAGE);
        common::ensure_directory(self.backend, &root, "缓存登记目录")?;
        if root.join("registration.json").exists() {
            let (request, binding) = self.registration()?;
            if let Some(filename) = filename {
                common::common_file(self.backend, filename, "缓存请求")?;
                if doc::read(filename)? != request {
                    return Err(rejected("不能替换同一缓存运行的固定请求"));
                }
            }
            return Ok((request, binding));
        }
        let filename = filename.ok_or("首次缓存恢复必须显式提供固定请求")?;
        common::common_file(self.backend, filename, "缓存请求")?;
        let binding = common::binding(filename)?;
        let request = common::read_bound(self.backend, &binding, "缓存请求")?;
        self.validate(&request, &Value::Null)?;
        common::bound_file(self.backend, &binding, "缓存请求")?;
        doc::write_new(&root.join("request.json"), &request)?;
        let binding = common::binding(&root.join("request.json"))?;
        doc::write_new(
            &root.join("registration.json"),
            &json!({"request":binding,"manifest":common::binding(&self.directory.join("manifest.json"))?}),
        )?;
        Ok((request, binding))
    }
}
fn registration(backend: &Path, directory: &Path) -> Result<(Value, Value)> {
    let root = directory.join(STAGE);
    common::local_directory(backend, &root, "缓存登记目录")?;
    let value = doc::read(&root.join("registration.json"))?;
    doc::exact(&value, &["request", "manifest"], "缓存登记")?;
    if value["manifest"] != common::binding(&directory.join("manifest.json"))?
        || path(&value["request"]["path"])? != root.join("request.json")
    {
        return Err(rejected("缓存登记不属于固定运行"));
    }
    let request = common::read_bound(backend, &value["request"], "缓存请求")?;
    if request["manifest"] != value["manifest"] || request["side"] != "target" {
        return Err(rejected("缓存请求不属于原目标"));
    }
    Ok((request, value["request"].clone()))
}

pub(super) fn require_live_request(
    backend: &Path,
    directory: &Path,
    supplied: Option<&Path>,
) -> Result<()> {
    if directory
        .join(STAGE)
        .join("registration.json")
        .try_exists()?
    {
        let (request, _) = registration(backend, directory)?;
        return super::require_windows(&request["process"]);
    }
    if let Some(supplied) = supplied {
        let file = common::common_file(backend, supplied, "缓存阶段输入")?;
        let request = doc::read(&file)?;
        super::require_windows(&request["process"])?;
    }
    Ok(())
}

pub(super) fn path(value: &Value) -> Result<PathBuf> {
    Ok(PathBuf::from(doc::text(value, "缓存路径")?))
}
pub(super) fn number(value: &Value) -> Result<u64> {
    value["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| "缓存阶段序号无效".into())
}
pub(super) fn dispatch(backend: &Path, args: &Value, runtime: &mut dyn Runtime) -> Result<Value> {
    doc::exact(
        args,
        &["action", "directory", "caller", "parameters"],
        "缓存阶段参数",
    )?;
    let directory = common::local_directory(backend, &path(&args["directory"])?, "缓存运行目录")?;
    let caller = args["caller"]
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v > 1)
        .ok_or("缓存调用者 PID 无效")?;
    let mut context = Context {
        backend,
        directory,
        caller,
        runtime,
    };
    let p = &args["parameters"];
    match doc::text(&args["action"], "缓存阶段动作")? {
        "execute" => execute::run(&mut context, p),
        "execution-preflight" => execute::preflight(&context, p),
        "guard" => execute::guard(&mut context, p).map(|()| Value::Null),
        "registration" => {
            doc::exact(p, &[], "缓存登记参数")?;
            let (request, descriptor) = context.registration()?;
            Ok(json!([request, descriptor]))
        }
        "starts" => {
            doc::exact(
                p,
                &["request", "descriptor", "before", "frozen"],
                "缓存创建参数",
            )?;
            Ok(json!(history::starts(
                &mut context,
                &p["request"],
                &p["descriptor"],
                p["before"].as_u64(),
                &p["frozen"]
            )?))
        }
        "reboot-starts" => {
            doc::exact(
                p,
                &["request", "descriptor", "before"],
                "重启原缓存创建参数",
            )?;
            super::require_windows(&p["request"]["process"])?;
            if context.registration()? != (p["request"].clone(), p["descriptor"].clone()) {
                return Err(rejected("重启原缓存创建必须使用同一冻结登记"));
            }
            let before = if p["before"].is_null() {
                None
            } else {
                Some(number(&json!({"number":p["before"]}))?)
            };
            Ok(json!(history::starts_with_tools(
                &mut context,
                &p["request"],
                &p["descriptor"],
                before,
                &Value::Null,
                false,
            )?))
        }
        "reboot-generation" => {
            doc::exact(p, &["request", "output"], "重启原缓存代次参数")?;
            let (request, _) = context.registration()?;
            super::require_windows(&request["process"])?;
            if request != p["request"] {
                return Err(rejected("重启原缓存代次必须使用同一冻结登记"));
            }
            let (process, tools) = successor::generation(
                &mut context,
                &request,
                &path(&p["output"])?,
                &Value::Null,
                false,
            )?;
            Ok(json!([process, tools]))
        }
        "generation" => {
            doc::exact(
                p,
                &[
                    "request",
                    "output",
                    "owned_lock_identity",
                    "observe_current_tools",
                ],
                "缓存代次参数",
            )?;
            let (process, tools) = successor::generation(
                &mut context,
                &p["request"],
                &path(&p["output"])?,
                &p["owned_lock_identity"],
                doc::boolean(&p["observe_current_tools"], "观察当前工具")?,
            )?;
            Ok(json!([process, tools]))
        }
        "binding" => history::registered(&mut context, p),
        "status" => history::status(&mut context, p),
        "preflight" => successor::preflight(&mut context, p),
        "history-proof" => successor::history_proof(&mut context, p),
        "restart-guard" => successor::restart_guard(&mut context, p).map(|()| Value::Null),
        _ => Err(rejected("未知缓存阶段动作")),
    }
}

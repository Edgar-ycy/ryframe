//! 原缓存账本的 binding 查询适配；不开放写动作或残余 Python 端口。
use super::{
    super::{json as document, resources::common, target_verify},
    evidence, session, stage,
};
#[path = "readonly/request.rs"]
mod request;
#[path = "readonly/source.rs"]
pub(crate) mod source;
#[path = "readonly/tools.rs"]
mod tools;
#[path = "binding/tools.rs"]
mod wsl;
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;

pub(crate) fn load(
    backend: &Path,
    run: &Path,
    owner: Option<&target_verify::guard::Generation>,
) -> Result<Value> {
    query(backend, run, owner, &Value::Null, &Value::Null)
}

pub(crate) fn query(
    backend: &Path,
    run: &Path,
    owner: Option<&target_verify::guard::Generation>,
    before: &Value,
    frozen: &Value,
) -> Result<Value> {
    let args = json!({"action":"binding","directory":run,"caller":std::process::id(),
        "parameters":{"owned_lock_identity":null,"before":before,"frozen":frozen}});
    execute_stage(backend, &args, owner)
}

pub(crate) fn execute_stage(
    backend: &Path,
    args: &Value,
    owner: Option<&target_verify::guard::Generation>,
) -> Result<Value> {
    let run = Path::new(document::text(&args["directory"], "缓存发布查询目录")?);
    common::local_directory(backend, run, "缓存发布查询目录")?;
    let scope = super::super::bridge::scope::Scope::load(backend, &[], &[])?;
    let state_path = run.join("state.json");
    let state = common::binding(&state_path)?;
    let manifest = common::binding(&run.join("manifest.json"))?;
    let mut runtime = BindingRuntime {
        backend,
        run,
        owner,
    };
    let result = stage::dispatch(backend, args, &mut runtime);
    scope.assert_unchanged()?;
    if common::binding(&state_path)? != state
        || common::binding(&run.join("manifest.json"))? != manifest
    {
        return Err("缓存发布查询期间完整账本或清单变化".into());
    }
    result
}

struct BindingRuntime<'a> {
    backend: &'a Path,
    run: &'a Path,
    owner: Option<&'a target_verify::guard::Generation>,
}

impl session::Runtime for BindingRuntime<'_> {
    fn call(&mut self, operation: &str, arguments: Value) -> Result<Value> {
        match operation {
            "status" | "predecessor" => super::readonly::port(self.backend, operation, &arguments),
            "validate" => {
                document::exact(
                    &arguments,
                    &["request", "owned_lock_identity"],
                    "缓存只读请求校验",
                )?;
                request::validate(self.backend, self.run, &arguments["request"], self.owner)
            }
            "inspect" => {
                document::exact(&arguments, &["request", "output"], "缓存实际创建证据")?;
                let path = Path::new(document::text(&arguments["output"], "缓存创建目录")?);
                common::local_directory(self.backend, path, "缓存创建目录")?;
                evidence::inspect(&arguments["request"], path)
            }
            "successor" => self.successor(&arguments),
            _ => Err("缓存 binding 查询拒绝资源控制和未知端口；保留现场".into()),
        }
    }
}

impl BindingRuntime<'_> {
    fn successor(&self, args: &Value) -> Result<Value> {
        document::exact(
            args,
            &[
                "request",
                "successor",
                "owned_lock_identity",
                "observe_current_tools",
            ],
            "缓存只读工具后继",
        )?;
        let source = source::load(self.backend, &args["successor"])?;
        let relation = &source["review_successor"];
        let descriptor = &relation["successor_review"];
        let review = common::read_bound(
            self.backend,
            &json!({"path":descriptor["path"],
            "bytes":descriptor["bytes"],"sha256":descriptor["sha256"]}),
            "缓存后继审阅",
        )?;
        let observed = tools::observe(
            self.backend,
            &review,
            &args["request"]["process"],
            document::boolean(&args["observe_current_tools"], "观察当前工具")?,
        )?;
        let effective = super::request::run(
            "successor-model",
            &json!({"request":args["request"],
            "directory":self.run,"value":document::read(&self.run.join("manifest.json"))?,
            "source_directory":source["directory"],"source_manifest":source["manifest"],"tools":observed,
            "tool_wsl_path":canonical_tool(&observed["wsl"]["path"])?,
            "original_wsl_path":canonical_tool(&args["request"]["process"]["wsl"]["path"])?,}),
        )?;
        let private = request::successor(
            self.backend,
            self.run,
            &args["request"],
            &source,
            &effective,
            document::boolean(&args["observe_current_tools"], "观察当前工具")?,
        )?;
        Ok(json!([effective, private, source]))
    }
}

fn canonical_tool(value: &Value) -> Result<std::path::PathBuf> {
    target_verify::input::request_configuration::canonical_path(Path::new(document::text(
        value,
        "WSL 工具路径",
    )?))
}

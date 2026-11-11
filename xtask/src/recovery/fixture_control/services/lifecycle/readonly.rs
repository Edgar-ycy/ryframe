//! fresh-target 使用的只读服务端口；不授予服务启动、停止或恢复权限。
use super::{Runtime, context, model, registration};
use crate::{
    Result,
    recovery::clone::native::{json as document, resources::common},
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[path = "readonly/process.rs"]
pub(crate) mod process;
#[path = "readonly/tree.rs"]
mod tree;

const FIXTURE_MEMBERS: &[&str] = &["rustfs", "api", "worker"];

pub(in crate::recovery) fn restore_completion(backend: &Path, tree: &Value) -> Result<Value> {
    tree::members(
        backend,
        tree,
        Duration::from_secs(10),
        &["api", "worker", "frontend"],
    )
}

pub(in crate::recovery) fn check_restore_completion(expected: &Value, proof: &Value) -> Result<()> {
    if proof["format_version"].as_u64() != Some(1) {
        return Err("恢复运行成员关闭证明版本无效".into());
    }
    tree::validate_members(expected, proof)
}

pub(super) fn handles(operation: &str) -> bool {
    matches!(
        operation,
        "review-successor"
            | "cache-status"
            | "cache-handoff-markers"
            | "cache-external-termination"
            | "storage-running"
            | "storage-closed"
            | "members-wait"
            | "controller"
            | "identity"
            | "completion"
            | "tree-read"
    )
}

pub(crate) struct NativeRuntime {
    backend: PathBuf,
    started: Instant,
}

impl NativeRuntime {
    pub(crate) fn new(backend: &Path) -> Self {
        Self {
            backend: backend.to_owned(),
            started: Instant::now(),
        }
    }

    pub(crate) fn observe_controller(&mut self, run: &Path) -> Result<Value> {
        self.primitive("controller", json!({"run":run}))
    }

    pub(crate) fn complete_registered_tree(&mut self, tree: &Value) -> Result<Value> {
        self.primitive("completion", json!({"tree":tree}))
    }

    pub(crate) fn observe_listener(&mut self, identity: &Value, url: &str) -> Result<()> {
        common::validate_process_identity(identity)?;
        let actual = self.primitive("identity", json!({"pid":identity["pid"]}))?;
        if actual != *identity {
            return Err("登记 API 进程已退出或内核创建身份变化".into());
        }
        process::listener(identity, url)
    }

    fn primitive(&mut self, operation: &str, args: Value) -> Result<Value> {
        super::primitive::validate(&self.backend, operation, &args)?;
        match operation {
            "cache-status" => crate::recovery::clone::native::cache::readonly::status(
                &self.backend, &args["request"], &args["runtime"],
                (!args["effective"].is_null()).then_some(&args["effective"]))
                .map_err(|error| {
                    let reason = crate::recovery::clone::native::cache::policy_message(error.as_ref());
                    model::error(reason.unwrap_or("缓存原生观察失败。核对 Windows Redis 工具绑定、冻结收据、进程代次和端口占用，保留现场，禁止自动重放"))
                }),
            "cache-external-termination" =>
                crate::recovery::clone::native::cache::readonly::external_termination(
                    &self.backend, &args["request"], &args["runtime"],
                    (!args["effective"].is_null()).then_some(&args["effective"]),
                )
                .map_err(|_| model::error("Windows Redis 外部终止证据核对失败。核对冻结收据、原进程身份和独占回环端口，保留现场，禁止自动重放")),
            "cache-handoff-markers" => crate::recovery::clone::native::cache::readonly::handoff_markers(
                &self.backend, &args["request"], &args["runtime"], &args["markers"],
            )
            .map_err(|_| model::error("RustFS 移交无法核对原 Redis 代次及 owner/sentinel；保留现场")),
            "review-successor" => {
                let (code, result) =
                    crate::recovery::fixture_control::environment::protocol::execute(
                        &serde_json::to_vec(
                            &json!({"format_version":1,"operation":"preflight-successor",
                        "arguments":args,"write":false}),
                        )?,
                    );
                if code != 0 || result["ok"] != true {
                    return Err(model::error(
                        "夹具继承审阅不满足原始预检关系；核对前驱与当前审阅绑定",
                    ));
                }
                Ok(Value::Null)
            }
            "identity" => {
                let pid = args["pid"]
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or("进程 PID 无效")?;
                common::process_identity(pid)
                    .map(|value| value.unwrap_or(Value::Null))
                    .map_err(|error| {
                        if crate::recovery::clone::native::lifecycle::identity::is_access_denied(
                            error.as_ref(),
                        ) || error.downcast_ref::<std::io::Error>().is_some_and(|error| {
                            error.kind() == std::io::ErrorKind::PermissionDenied
                        }) {
                            Box::new(model::LifecycleError {
                                category: "PermissionError",
                                message: "读取登记进程身份被拒绝访问；不能推断已退出",
                            }) as Box<dyn std::error::Error>
                        } else {
                            error
                        }
                    })
            }
            "tree-read" => tree::read(
                &self.backend,
                Path::new(document::text(&args["output"], "进程树目录")?),
                document::text(&args["scope_id"], "进程树 scope")?,
            ),
            "members-wait" => {
                let binding = tree::members(&self.backend, &args["tree"], Duration::ZERO, FIXTURE_MEMBERS)?;
                common::read_bound(&self.backend, &binding, "完整成员关闭证明")
            }
            "completion" => tree::members(&self.backend, &args["tree"], Duration::from_secs(10), FIXTURE_MEMBERS),
            "controller" => self.controller(&args),
            "storage-running" => self.storage(&args),
            "storage-closed" => {
                super::observation::closed(&args["request"])?;
                Ok(json!(true))
            }
            _ => Err(model::error(
                "只读服务观察不接受资源控制操作；请使用正式生命周期入口",
            )),
        }
    }

    fn controller(&self, args: &Value) -> Result<Value> {
        let run = Path::new(document::text(&args["run"], "控制器目录")?);
        let filename = run.join("state.json");
        let before = common::binding(&filename)?;
        let saved = crate::recovery::clone::native::lifecycle::state::load(run, true)?;
        let result =
            crate::recovery::clone::native::lifecycle::status::observe_controller(run, &saved)?;
        if common::binding(&filename)? != before {
            return Err(model::error("控制状态完整字节在只读观察期间变化"));
        }
        Ok(result)
    }

    fn storage(&self, args: &Value) -> Result<Value> {
        let identity = &args["identity"];
        let request = &args["request"];
        common::validate_process_identity(identity)?;
        let pid = u32::try_from(identity["pid"].as_u64().ok_or("RustFS PID 无效")?)?;
        let Some(actual) = common::process_identity(pid)? else {
            return Ok(json!(false));
        };
        if actual != *identity {
            return Err(model::error("存储 PID 已复用，拒绝采用其他内核代次"));
        }
        let expected =
            crate::recovery::clone::native::resources::storage::model::arguments(request)?;
        if process::arguments(identity)? != expected {
            return Err(model::error("RustFS 实际参数不同于冻结请求"));
        }
        for key in ["api_url", "console_url"] {
            process::listener(identity, document::text(&request[key], "RustFS 监听地址")?)?;
        }
        process::require(identity)?;
        Ok(json!(true))
    }
}

impl Runtime for NativeRuntime {
    fn call(&mut self, operation: &str, args: Value) -> Result<Value> {
        let backend = self.backend.clone();
        match operation {
            "observe" => {
                document::exact(&args, &["services"], "只读服务观察")?;
                super::observation::observe(self, &backend, &args["services"])
            }
            "context" => {
                document::exact(&args, &["review", "environment"], "只读服务上下文")?;
                context::load(
                    self,
                    &backend,
                    Path::new(document::text(&args["review"], "审阅文件")?),
                    Path::new(document::text(&args["environment"], "环境文件")?),
                )
            }
            "registered" => {
                document::exact(&args, &["context", "settled"], "只读服务登记")?;
                registration::load(
                    self,
                    &backend,
                    &args["context"],
                    document::boolean(&args["settled"], "settled")?,
                )
            }
            _ => self.primitive(operation, args),
        }
    }
    fn now(&self) -> Duration {
        self.started.elapsed()
    }
    fn pause(&mut self, delay: Duration) {
        std::thread::sleep(delay);
    }
}

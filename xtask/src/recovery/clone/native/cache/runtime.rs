//! 同一缓存阶段直接调用现有 Rust 权威；不委托解释器执行资源操作。
use super::{binding, session::Runtime, stage};
use crate::{
    Result,
    recovery::clone::native::{json as document, target_verify},
};
use serde_json::{Value, json};
use std::path::Path;

mod producers;
mod redis;

pub(super) struct Native<'a> {
    pub backend: &'a Path,
    pub directory: &'a Path,
    pub owner: Option<&'a target_verify::guard::Generation>,
}

impl Native<'_> {
    pub(super) fn dispatch(&mut self, action: &str, parameters: Value) -> Result<Value> {
        stage::dispatch(
            self.backend,
            &json!({"action":action,"directory":self.directory,
            "caller":std::process::id(),"parameters":parameters}),
            self,
        )
    }
}

impl Runtime for Native<'_> {
    fn call(&mut self, operation: &str, arguments: Value) -> Result<Value> {
        match operation {
            "checkpoint" => self.dispatch("guard", arguments),
            "validate" | "successor" | "inspect" | "status" | "predecessor" => binding::port(
                self.backend,
                self.directory,
                self.owner,
                operation,
                arguments,
            ),
            "start" => {
                document::exact(
                    &arguments,
                    &["request", "private", "output", "guard"],
                    "缓存启动端口",
                )?;
                let backend = self.backend;
                super::host_start::start(
                    backend,
                    &arguments["request"],
                    &arguments["private"],
                    Path::new(document::text(&arguments["output"], "缓存启动目录")?),
                    || {
                        self.call("checkpoint", arguments["guard"].clone())
                            .map(|_| ())
                    },
                )
            }
            "observe" => {
                document::exact(
                    &arguments,
                    &["request", "private", "runtime"],
                    "缓存观察端口",
                )?;
                super::host_control::observe(
                    self.backend,
                    &arguments["request"],
                    &arguments["runtime"],
                )
            }
            "stop" => {
                document::exact(
                    &arguments,
                    &["request", "private", "runtime", "output"],
                    "缓存停止端口",
                )?;
                super::host_control::stop(
                    self.backend,
                    &arguments["request"],
                    &arguments["runtime"],
                    Path::new(document::text(&arguments["output"], "缓存停止目录")?),
                )
            }
            "redis" => redis::command(self, &arguments),
            "producers-stopped" => producers::stopped(self),
            "quiet" => crate::recovery::clone::native::seed::protocol::guards::rebind::live::quiet(
                self.backend,
                &json!({"registration":arguments["registration"],"request":arguments["request"]}),
            ),
            "seed-history" | "seed-segment" | "restart-authorization" => {
                crate::recovery::source::evidence::cache_history(
                    self.backend,
                    self.directory,
                    operation,
                    &arguments,
                )
            }
            _ => Err(super::rejected(
                "缓存阶段拒绝未知资源端口；保留现场，禁止重放",
            )),
        }
    }
}

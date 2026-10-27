//! 原生全栈阶段协调；既有进程树和来源权威只作为底层核验端口复用。
#[path = "full_stack/build.rs"]
pub(in crate::recovery) mod build;
#[path = "full_stack/context.rs"]
pub(super) mod context;
#[path = "full_stack/execution.rs"]
pub(super) mod execution;
#[path = "full_stack/lifecycle.rs"]
mod lifecycle;
#[path = "full_stack/prepare.rs"]
mod prepare;
#[path = "full_stack/provenance.rs"]
pub(in crate::recovery) mod provenance;
#[cfg(test)]
#[path = "../../tests/unit/recovery/full_stack/tests.rs"]
mod tests;
#[path = "full_stack/worker/mod.rs"]
pub(crate) mod worker;

use crate::{Result, cli::FullStackCommand};
use serde_json::Value;
use std::{path::Path, time::Duration};

pub(super) trait Port {
    fn leaf(&mut self, operation: &str) -> Result<Value>;
    fn command(&mut self, program: &str, args: &[String], log: Option<&Path>) -> Result<String>;
    fn command_at(
        &mut self,
        _cwd: &Path,
        program: &str,
        args: &[String],
        log: Option<&Path>,
    ) -> Result<String> {
        self.command(program, args, log)
    }
    fn ready(&mut self, url: &str) -> Result<bool>;
    fn sleep(&mut self, duration: Duration) -> Result<()>;
    fn storage(&mut self, table: &Value) -> Result<()>;
    fn databases(&mut self, frozen: &crate::ci::databases::snapshot::Frozen) -> Result<()>;
}

pub(crate) fn run(command: &FullStackCommand, root: &Path) -> Result<()> {
    match command {
        FullStackCommand::Help => {
            println!("cargo xtask check recovery full-stack <prepare|rate-limit|start|collect>");
            return Ok(());
        }
        FullStackCommand::RateLimitHelp => {
            println!(
                "cargo xtask check recovery full-stack rate-limit --tool-config-file <绝对文件>"
            );
            return Ok(());
        }
        _ => {}
    }
    let context = context::Context::load(root)?;
    let mut port = execution::Native::new(&context)?;
    match command {
        FullStackCommand::Prepare => prepare::run(&context, &mut port),
        FullStackCommand::Start => lifecycle::start(&context, &mut port),
        FullStackCommand::Collect => lifecycle::collect(&context, &mut port),
        FullStackCommand::RateLimit { tool_config_file } => context.rate_limit(tool_config_file),
        _ => unreachable!(),
    }
}

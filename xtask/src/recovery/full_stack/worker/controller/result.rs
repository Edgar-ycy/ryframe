//! 在同一控制器预算内收取原启动结果，不重放启动或停止。
use super::super as worker;
use super::{Operation, Pending, archive_completed, guard, operation, process, tree};
use crate::Result;
use serde_json::Value;
use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};

pub(super) fn finish(
    backend: &Path,
    directory: &Path,
    runtime: &Value,
    source: &str,
    op: &Operation,
    deadline: Instant,
) -> Result<Value> {
    let role = op.role;
    loop {
        worker::check_result_deadline(role, deadline)?;
        if op.path.join("result.json").exists() {
            let held = guard(
                role,
                directory,
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(10)),
            );
            worker::check_result_deadline(role, deadline)?;
            let guard = held?;
            worker::runtime::load(role, backend, directory)?;
            let current_source = operation::source(directory)?;
            worker::check_result_deadline(role, deadline)?;
            if current_source != source {
                return Err(
                    worker::Rejection(format!("{}等待期间来源发生变化", role.label())).into(),
                );
            }
            let current = Operation::read(role, directory, source)?;
            worker::check_result_deadline(role, deadline)?;
            if current.id()? != op.id()? {
                return Err(
                    worker::Rejection(format!("{}等待期间操作发生变化", role.label())).into(),
                );
            }
            let completed = archive_completed(&current);
            worker::check_result_deadline(role, deadline)?;
            let value = match completed {
                Err(error) if error.is::<Pending>() => {
                    drop(guard);
                    thread::sleep(
                        Duration::from_millis(20)
                            .min(deadline.saturating_duration_since(Instant::now())),
                    );
                    continue;
                }
                result => result?,
            };
            guard.check()?;
            worker::check_result_deadline(role, deadline)?;
            if value["outcome"] != "succeeded" {
                return Err(worker::Rejection(format!(
                    "{}启动失败；已保留原始失败收据和完整成员关闭证明",
                    role.label()
                ))
                .into());
            }
            tree::running(directory, runtime, role, &value["identity"])?;
            worker::check_result_deadline(role, deadline)?;
            process::listener(
                &value["identity"],
                runtime[role.ready()].as_str().ok_or("端点无效")?,
            )?;
            worker::check_result_deadline(role, deadline)?;
            return Ok(value);
        }
        thread::sleep(
            Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

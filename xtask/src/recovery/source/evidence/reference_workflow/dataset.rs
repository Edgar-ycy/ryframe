//! 参考数据准备持有原工作目录锁，写入阶段只启动一次原生子进程。
use std::{fs, io::ErrorKind, path::Path};

use serde_json::{Value, json};

use super::{files, local::Lock, native, path, readonly, receipts, unlinked};
use crate::{Result, recovery::monitoring::execution::ProtectedInputs};

mod binding;
mod child;

use binding::Bound;

pub(super) fn run(backend: &Path, request: &Value, plan: &Value) -> Result<Value> {
    let bound = Bound::capture(backend, request, plan)?;
    let work = files::work(&json!({"plan":plan}), true)?;
    let work = path(&work)?;
    let mut lock = Lock::claim(work)?;
    for name in [
        "dataset.json",
        "dataset-tools.json",
        "dataset-preflight.json",
        "dataset",
    ] {
        require_absent(&work.join(name))?;
    }
    let started = receipts::start(&json!({
        "plan":plan,"command":"dataset","copy_id":null,"root":null,"receipt":null,"bindings":null,
    }))?;
    let stage = Stage {
        backend,
        work,
        lock: &lock,
        started: &started,
        plan,
    };
    let result = stage.execute(request, &bound);
    let result = stage.finish(result);
    if result.as_ref().is_err_and(|error| {
        error
            .downcast_ref::<child::ChildFailure>()
            .is_some_and(|failure| !failure.confirmed_closed)
    }) {
        lock.retain();
    }
    result
}

struct Stage<'a> {
    backend: &'a Path,
    work: &'a Path,
    lock: &'a Lock,
    started: &'a Value,
    plan: &'a Value,
}

impl Stage<'_> {
    fn check(&self) -> Result<()> {
        self.lock.check()?;
        receipts::check_started(self.started)?;
        if path(&files::work(&json!({"plan":self.plan}), false)?)? != self.work {
            return Err("参考数据准备目录不再属于原计划".into());
        }
        Ok(())
    }

    fn execute(&self, request: &Value, bound: &Bound) -> Result<Value> {
        self.check()?;
        let tools = self.work.join("dataset-tools.json");
        let preflight = self.work.join("dataset-preflight.json");
        native::json::write_new(&tools, &bound.tools_receipt)?;
        let mut read = request.clone();
        read["command"] = json!("check-dataset");
        read["write"] = json!(false);
        readonly::run(&read)?;
        self.check()?;
        native::json::write_new(&preflight, &bound.preflight)?;
        let written = ProtectedInputs::open([tools.clone(), preflight.clone()])?;
        check_document(&tools, &bound.tools_receipt)?;
        check_document(&preflight, &bound.preflight)?;
        bound.check()?;
        written.assert_unchanged()?;
        self.check()?;
        let directory = self.work.join("dataset");
        require_absent(&directory)?;
        child::run(self.backend, &bound.plan_path, &preflight, bound.timeout)?;
        let output = directory.join("result.json");
        unlinked(&output)?;
        let produced = ProtectedInputs::open([output.clone()])?;
        let mut result = native::json::read(&output)?;
        self.check()?;
        bound.check()?;
        written.assert_unchanged()?;
        produced.assert_unchanged()?;
        check_document(&tools, &bound.tools_receipt)?;
        check_document(&preflight, &bound.preflight)?;
        check_result(&result, &bound.preflight, self.plan)?;
        result["source"] = bound.source.clone();
        Ok(result)
    }

    fn finish(&self, result: Result<Value>) -> Result<Value> {
        match result {
            Ok(value) => {
                self.check()?;
                receipts::finish(&json!({
                    "receipt":self.started,"result":value,"failure":null,"resource_images":null,
                }))?;
                Ok(value)
            }
            Err(error) => {
                let mut failure = json!({
                    "error_type":"ReferenceDatasetFailure",
                    "message":"参考数据准备未完成；保留原预检、阶段与写入证据，禁止覆盖或自动重放",
                });
                if let Some(child) = error.downcast_ref::<child::ChildFailure>() {
                    failure["notes"] = json!([child.to_string()]);
                }
                let recorded = self.check().and_then(|()| {
                    receipts::finish(&json!({
                        "receipt":self.started,"result":null,"failure":failure,"resource_images":null,
                    }))
                    .map(|_| ())
                });
                if recorded.is_err() {
                    // 返回同一类型的子进程失败，调用方才能保留关闭未确认的原锁。
                    eprintln!("参考数据准备的失败收据发布未确认，保留运行收据和原现场");
                }
                Err(error)
            }
        }
    }
}

fn require_absent(path: &Path) -> Result<()> {
    unlinked(path)?;
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("参考数据准备输出已存在，保留原现场并拒绝重放".into()),
    }
}

fn check_document(path: &Path, expected: &Value) -> Result<()> {
    if native::json::read(path)? != *expected {
        return Err("参考数据准备预检或工具原收据发生变化".into());
    }
    Ok(())
}

fn check_result(result: &Value, preflight: &Value, plan: &Value) -> Result<()> {
    if result["format_version"] != 1
        || result["plan_sha256"] != preflight["plan_sha256"]
        || result["source_scope_id"] != preflight["scope_id"]
        || result["records"] != plan["dataset"]["records"]
        || result["request_interval_ms"] != plan["dataset"]["request_interval_ms"]
        || result["post_concurrency"] != 11
        || result["tenants"]
            .as_array()
            .is_none_or(|tenants| tenants.len() != 11)
    {
        return Err("数据准备结果没有绑定原计划、scope 或完整租户矩阵".into());
    }
    Ok(())
}

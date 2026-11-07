//! cache 正式入口沿用统一阶段锁、来源收尾和缓存阶段原有状态机。
use super::{runtime::Native, stage};
use crate::{
    Result,
    cli::{CacheOperation, CacheOptions},
    recovery::clone::native::{
        bridge::scope::Scope,
        json as document,
        lifecycle::{
            lock::ProcessGuard,
            stage::{self as lifecycle, StageRequest, StageRunner},
        },
        resources::common,
    },
};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::BTreeSet, path::Path};

pub(crate) fn run(backend: &Path, options: &CacheOptions) -> Result<Value> {
    common::local_directory(backend, &options.run_dir, "缓存运行目录")?;
    if options.operation == CacheOperation::Status {
        if options.request.is_some() {
            return Err("缓存 status 不能更换固定请求".into());
        }
        return super::binding::execute_stage(
            backend,
            &json!({"action":"status","directory":options.run_dir,"caller":std::process::id(),
                "parameters":{"allow_close_attempt":false}}),
            None,
        );
    }
    let value = lifecycle::run_named(
        backend,
        &options.run_dir,
        stage::STAGE,
        options.operation.as_str(),
        &Runner {
            options,
            successor: RefCell::new(None),
            scope: RefCell::new(None),
        },
    )?;
    Ok(
        json!({"status":value["status"],"stage":value["stage"],"mode":value["mode"],
        "attempt":value["attempt"],"restore_qualified":value["restore_qualified"]}),
    )
}

struct Runner<'a> {
    options: &'a CacheOptions,
    successor: RefCell<Option<Value>>,
    scope: RefCell<Option<Scope>>,
}

impl StageRunner for Runner<'_> {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        self.require(request, false)?;
        let mut runtime = Native {
            backend: request.backend,
            directory: request.run_dir,
            owner: None,
        };
        if !matches!(
            self.options.operation,
            CacheOperation::Stop | CacheOperation::Recover
        ) {
            let successor = runtime.dispatch(
                "preflight",
                json!({"filename":self.options.request,
                "mode":self.options.operation.as_str()}),
            )?;
            let mut frozen = self.successor.borrow_mut();
            match &*frozen {
                Some(expected) if *expected != successor => {
                    return Err("缓存后继在取得运行锁前变化".into());
                }
                None => *frozen = Some(successor),
                _ => (),
            }
            if frozen.as_ref().is_some_and(Value::is_null) {
                let mut scope = self.scope.borrow_mut();
                if let Some(scope) = scope.as_ref() {
                    scope.assert_unchanged()?;
                } else {
                    let manifest = document::read(&request.run_dir.join("manifest.json"))?;
                    *scope = Some(Scope::load(
                        request.backend,
                        document::array(&manifest["build_bridges"], "缓存阶段构建桥接")?,
                        &inherited_bridges(request.backend, &manifest)?,
                    )?);
                }
            }
        } else if self.options.request.is_some() {
            return Err("缓存清理不能替换固定请求".into());
        }
        Ok(())
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        self.require(request, true)?;
        let mut runtime = Native {
            backend: request.backend,
            directory: request.run_dir,
            owner: None,
        };
        let manifest = document::read(&request.run_dir.join("manifest.json"))?;
        let parameters = json!({"value":manifest,
            "mode":request.mode,"number":request.attempt,"request_file":self.options.request});
        runtime.dispatch("execution-preflight", parameters.clone())?;
        let _guard = ProcessGuard::acquire_named(request.run_dir, "cache-target.guard")?;
        let scope = self.scope.borrow();
        if let Some(scope) = scope.as_ref() {
            scope.assert_unchanged()?;
        }
        let result = runtime.dispatch("execute", parameters);
        if let Some(scope) = scope.as_ref() {
            scope.assert_unchanged()?;
        }
        result
    }
}

pub(crate) fn inherited_bridges(backend: &Path, manifest: &Value) -> Result<Vec<Value>> {
    if manifest["copy_stage"] != "seed_to_arm" {
        return Ok(vec![]);
    }
    let source = crate::recovery::source::evidence::comparison::copy_source(backend, manifest)?;
    if source["source_request"] != manifest["source_request"] {
        return Err("缓存继承来源不属于当前清单的源请求".into());
    }
    if !source["source_generation"].is_null() {
        return Ok(vec![]);
    }
    let expected = ["backend_build", "maintenance_build"]
        .map(|field| -> Result<_> {
            let value = &source["request"][field];
            Ok((
                document::text(&value["path"], "继承构建路径")?.to_owned(),
                document::text(&value["sha256"], "继承构建摘要")?.to_owned(),
            ))
        })
        .into_iter()
        .collect::<Result<BTreeSet<_>>>()?;
    let bridges = document::array(&source["manifest"]["build_bridges"], "继承构建桥接")?;
    let actual = bridges
        .iter()
        .map(|descriptor| -> Result<_> {
            let bridge = common::read_bound(backend, descriptor, "继承构建桥接")?;
            let build = &bridge["build"];
            document::exact(build, &["path", "sha256"], "继承构建收据")?;
            Ok((
                document::text(&build["path"], "继承构建路径")?.to_owned(),
                document::text(&build["sha256"], "继承构建摘要")?.to_owned(),
            ))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    if expected.len() != 2 || bridges.len() != 2 || actual != expected {
        return Err("缓存只能继承源请求的 API/Worker 与维护构建桥接".into());
    }
    Ok(bridges.to_vec())
}

impl Runner<'_> {
    fn require(&self, request: &StageRequest<'_>, executing: bool) -> Result<()> {
        if request.stage != stage::STAGE
            || request.mode != self.options.operation.as_str()
            || request.run_dir != self.options.run_dir
            || executing != request.attempt.is_some()
            || request.attempt == Some(0)
        {
            return Err("缓存阶段必须绑定同一正式操作、运行目录和控制器序号".into());
        }
        Ok(())
    }
}

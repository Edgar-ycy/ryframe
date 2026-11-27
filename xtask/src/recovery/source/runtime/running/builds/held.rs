//! Windows 专用来源窗口，在原件受保护时复用已确认的静态构建绑定。
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{common, document};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::{
        clone::native::target_verify::guard::Inputs,
        fixture_control::source_pair::execution::{self, HeldTargetSource},
        source::evidence::{generation_request::configuration, resources::warm_redis},
    },
};

pub(in crate::recovery::source::evidence) struct HeldBuilds {
    request: Value,
    declaration: Value,
    build: Value,
    source: HeldTargetSource,
    files: Inputs,
    failed_marker: PathBuf,
}

impl HeldBuilds {
    pub(in crate::recovery::source::evidence) fn open(
        backend: &Path,
        request: &Value,
        expected: &Value,
    ) -> Result<Self> {
        let declaration = declaration(request)?;
        let (execution, publication) = execution::target_binding(backend, &declaration)?;
        let source = HeldTargetSource::open(backend, &declaration, &publication)?;
        let bound = bound_files(backend, &execution, request, &publication)?;
        let maintenance_path =
            common::bound_file(&execution, &request["maintenance_build"], "来源维护构建")?;
        let directory = maintenance_path.parent().ok_or("来源维护构建目录缺失")?;
        let maintenance = document::read(&maintenance_path)?;
        let mut paths = bound.clone();
        paths.extend(product_files(expected)?);
        paths.extend(maintenance_files(&execution, directory, &maintenance)?);
        let window = Self {
            request: request.clone(),
            declaration,
            build: expected.clone(),
            source,
            files: Inputs::open(paths)?,
            failed_marker: directory.join("failed.json"),
        };
        // 校验打开前用于选择产物的收据仍与持有后的原件一致。
        if bound_files(backend, &execution, request, &publication)? != bound
            || document::read(&maintenance_path)? != maintenance
        {
            return Err("来源构建原件在打开保护窗口时发生变化".into());
        }
        window.checkpoint(backend, request)?;
        window.verify_complete(backend, request)?;
        Ok(window)
    }

    pub(in crate::recovery::source::evidence) fn checkpoint(
        &self,
        backend: &Path,
        request: &Value,
    ) -> Result<()> {
        if request != &self.request {
            return Err("核验请求与已保护的原构建窗口不同".into());
        }
        self.source.verify(backend, &self.declaration)?;
        configuration::verify(backend, request)?;
        self.files.check_identities()?;
        require_no_failed_marker(&self.failed_marker)
    }

    pub(in crate::recovery::source::evidence) fn checked_build(
        &self,
        backend: &Path,
        request: &Value,
    ) -> Result<&Value> {
        self.checkpoint(backend, request)?;
        Ok(&self.build)
    }

    pub(in crate::recovery::source::evidence) fn finish(
        &self,
        backend: &Path,
        request: &Value,
    ) -> Result<()> {
        let builds = self
            .checkpoint(backend, request)
            .and_then(|()| self.verify_complete(backend, request));
        let source = self.source.check_end();
        builds.and(source)?;
        self.checkpoint(backend, request)
    }

    fn verify_complete(&self, backend: &Path, request: &Value) -> Result<()> {
        if super::verify(backend, request)? != self.build {
            return Err("来源完整构建证据与原登记构建不同".into());
        }
        self.files.check()?;
        require_no_failed_marker(&self.failed_marker)
    }
}

fn declaration(request: &Value) -> Result<Value> {
    if !warm_redis::applies(request)
        || request.get("adapter_contract") != Some(&Value::Null)
        || request.get("product_backend") != Some(&Value::Null)
        || !request["execution_source"].is_object()
    {
        return Err("构建持有窗口仅适用于无适配的原暖态发布来源".into());
    }
    let path = document::text(&request["execution_backend"], "来源执行根")?;
    Ok(json!({"path":path,"execution_source":request["execution_source"]}))
}

fn bound_files(
    backend: &Path,
    execution: &Path,
    request: &Value,
    publication: &Value,
) -> Result<BTreeSet<PathBuf>> {
    [
        (backend, &request["backend_build"], "来源产品构建"),
        (execution, &request["maintenance_build"], "来源维护构建"),
        (backend, &request["source_environment"], "来源私有环境"),
        (backend, &request["source_registration"], "来源原登记"),
        (backend, &request["source_rebind"], "来源重绑定"),
        (backend, &request["review_successor"], "来源审阅后继"),
        (backend, &publication["execution_source"], "执行源码发布"),
        (backend, &publication["fixture"], "原夹具发布"),
    ]
    .into_iter()
    .map(|(root, binding, label)| common::bound_file(root, binding, label))
    .collect()
}

fn product_files(build: &Value) -> Result<Vec<PathBuf>> {
    let artifacts = document::exact(&build["artifacts"], &["api", "worker"], "来源产品产物")?;
    if build["format_version"].as_u64() != Some(2) || build["kind"] != "restore-backend-build" {
        return Err("来源持有窗口缺少原产品构建".into());
    }
    artifacts
        .values()
        .map(|artifact| {
            let path = Path::new(document::text(&artifact["executable"], "来源产品路径")?);
            Ok(validate_absolute_path(
                path,
                LocalTestPathKind::ExistingFile,
            )?)
        })
        .collect()
}

fn maintenance_files(execution: &Path, directory: &Path, build: &Value) -> Result<Vec<PathBuf>> {
    let artifacts = document::exact(
        &build["artifacts"],
        &["migrate", "reset", "tenant-data"],
        "来源维护产物",
    )?;
    if build["format_version"].as_u64() != Some(1) || build["kind"] != "devex-clone-tool-build" {
        return Err("来源持有窗口缺少原维护构建".into());
    }
    let mut paths = Vec::with_capacity(9);
    for (role, artifact) in artifacts {
        let executable = Path::new(document::text(&artifact["executable"], "来源维护工具路径")?);
        if executable.parent() != Some(directory) {
            return Err("维护工具必须在其原构建收据目录".into());
        }
        paths.push(common::common_file(execution, executable, "维护工具原件")?);
        for file in cargo_logs(role, artifact)? {
            paths.push(common::common_file(
                execution,
                &directory.join(file),
                "维护工具构建日志",
            )?);
        }
    }
    Ok(paths)
}

fn cargo_logs(role: &str, artifact: &Value) -> Result<[String; 2]> {
    let output = format!("{role}.cargo.jsonl");
    let log = format!("{role}.cargo.log");
    if artifact["cargo_output"]["file"] != output || artifact["cargo_log"]["file"] != log {
        return Err("维护工具 Cargo 日志绑定与原角色文件名不符".into());
    }
    Ok([output, log])
}

fn require_no_failed_marker(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("来源维护构建出现失败收据".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_builds_accept_only_the_original_published_warm_request() {
        let mut request = json!({
            "format_version":2,
            "id":"wkc61111h-current-20261127-r1",
            "source_registration":{"sha256":"4f084753d4c2b797d3aa73c967a3bab0d9051f8cc569536651e2eaed79166ff6"},
            "adapter_contract":null,
            "product_backend":null,
            "execution_backend":"D:/source/backend",
            "execution_source":{"path":"D:/source/execution-source.json"}
        });
        assert_eq!(
            declaration(&request).unwrap(),
            json!({"path":request["execution_backend"],"execution_source":request["execution_source"]})
        );
        for field in ["adapter_contract", "product_backend"] {
            request[field] = json!({});
            assert!(declaration(&request).is_err());
            request[field] = Value::Null;
        }
        request["source_registration"]["sha256"] = json!("a".repeat(64));
        assert!(declaration(&request).is_err());
    }

    #[test]
    fn held_builds_reject_cargo_logs_outside_the_role() {
        let mut artifact = json!({
            "cargo_output":{"file":"reset.cargo.jsonl"},
            "cargo_log":{"file":"reset.cargo.log"}
        });
        assert_eq!(
            cargo_logs("reset", &artifact).unwrap(),
            ["reset.cargo.jsonl", "reset.cargo.log"]
        );
        assert!(cargo_logs("migrate", &artifact).is_err());
        artifact["cargo_log"]["file"] = json!("../reset.cargo.log");
        assert!(cargo_logs("reset", &artifact).is_err());
    }
}

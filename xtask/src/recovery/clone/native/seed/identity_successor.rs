//! 身份和解只追加新的生产者登记。历史阶段继续使用其启动收据绑定的原登记。

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::clone::native::{json as document, lifecycle::state, resources::common},
};

pub(crate) const MODE: &str = "identities-reconcile";
const STATUS: &str = "seed_identity_create_reconciled";
const FILE: &str = "seed-runtime/identity-successor.json";

pub(crate) fn path(run: &Path) -> PathBuf {
    run.join(FILE)
}

pub(crate) fn publish(
    backend: &Path,
    run: &Path,
    original: &Value,
    tool: &Value,
    build: &Value,
    reconciliation: &Value,
) -> Result<Value> {
    let manifest = state::manifest_binding(run)?;
    let value = json!({
        "format_version": 1,
        "kind": "devex-seed-identity-successor",
        "run_manifest": manifest,
        "original_registration": original,
        "tool": tool,
        "build": build,
        "reconciliation": reconciliation,
    });
    checked(backend, run, &value)?;
    let filename = path(run);
    match fs::symlink_metadata(&filename) {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            document::write_new(&filename, &value)?
        }
        Err(error) => return Err(error.into()),
        Ok(_) => {
            let bound = common::binding(&filename)?;
            if common::read_bound(backend, &bound, "原后继身份生产者登记")? != value {
                return Err("已有后继身份生产者登记不同；保留原登记".into());
            }
        }
    }
    let successor = common::binding(&filename)?;
    if common::read_bound(backend, &successor, "后继身份生产者登记")? != value {
        return Err("后继身份生产者在发布期间变化".into());
    }
    Ok(json!({
        "status": STATUS,
        "registration": original,
        "identity_successor": successor,
        "identity_reconciliation": reconciliation,
        "remote_writes": 0,
        "worker_authorized": false,
        "restore_qualified": false,
    }))
}

pub(crate) fn select(
    backend: &Path,
    run: &Path,
    attempt: u64,
    original_request: &Value,
    original: &Value,
) -> Result<(Value, Value)> {
    let Some(binding) = authorized_binding(backend, run, Some(attempt))? else {
        return Ok((original_request.clone(), original.clone()));
    };
    let selected = resolve(backend, run, &binding, Some(attempt))?;
    let mut original_with_tool = original_request.clone();
    original_with_tool["identity_tool"] = selected["identity_tool"].clone();
    if selected != original_with_tool {
        return Err("身份后继登记改变了原计划或其它资源绑定".into());
    }
    Ok((selected, binding))
}

pub(crate) fn resolve(
    backend: &Path,
    run: &Path,
    descriptor: &Value,
    attempt: Option<u64>,
) -> Result<Value> {
    if authorized_binding(backend, run, attempt)?.as_ref() != Some(descriptor)
        || common::bound_file(backend, descriptor, "后继身份生产者登记")? != path(run)
    {
        return Err("身份后继工具没有已通过且在本阶段之前的和解授权".into());
    }
    let value = common::read_bound(backend, descriptor, "后继身份生产者登记")?;
    checked(backend, run, &value)
}

fn checked(backend: &Path, run: &Path, value: &Value) -> Result<Value> {
    document::exact(
        value,
        &[
            "format_version",
            "kind",
            "run_manifest",
            "original_registration",
            "tool",
            "build",
            "reconciliation",
        ],
        "后继身份生产者登记",
    )?;
    let original = run.join("seed-runtime.json");
    if value["format_version"] != 1
        || value["kind"] != "devex-seed-identity-successor"
        || value["run_manifest"] != state::manifest_binding(run)?
        || common::bound_file(backend, &value["original_registration"], "原身份生产者登记")?
            != original
        || common::bound_file(backend, &value["reconciliation"], "身份和解收据")?
            != run.join("seed-runtime/identities/first-create-reconciliation/first-create-reconciliation-result.json")
    {
        return Err("后继身份登记未绑定原运行、原登记和完整和解".into());
    }
    let original =
        common::read_bound(backend, &value["original_registration"], "原身份生产者登记")?;
    let build = common::read_bound(backend, &value["build"], "后继身份工具的构建证据")?;
    if build["kind"] != "devex-seed-identity-successor-build"
        || build["format_version"] != 1
        || build["tool"] != value["tool"]
        || value["tool"] == original["identity_tool"]
    {
        return Err("身份工具后继与原工具或独立构建证据不符".into());
    }
    document::exact(&value["tool"], &["path", "sha256"], "后继身份工具")?;
    let mut selected = original;
    selected["identity_tool"] = value["tool"].clone();
    Ok(selected)
}

fn authorized_binding(backend: &Path, run: &Path, attempt: Option<u64>) -> Result<Option<Value>> {
    let saved = state::load(run, false)?;
    let Some(record) = state::attempts(&saved)?.iter().rfind(|record| {
        record["stage"] == "seed-runtime"
            && record["mode"] == MODE
            && attempt.is_none_or(|number| record["number"].as_u64().is_some_and(|n| n < number))
    }) else {
        return Ok(None);
    };
    if record["status"] != "passed" {
        return Err("最近的身份和解仍未通过，拒绝接续生产者".into());
    }
    let value = common::read_bound(backend, &record["result"], "身份和解外层结果")?;
    let descriptor = &value["identity_successor"];
    if value["status"] != STATUS
        || value["registration"] != common::binding(&run.join("seed-runtime.json"))?
        || value["remote_writes"] != 0
        || value["worker_authorized"] != false
        || value["restore_qualified"] != false
        || common::bound_file(backend, descriptor, "后继身份生产者登记")? != path(run)
    {
        return Err("外层身份和解收据未授权后继工具".into());
    }
    let body = common::read_bound(backend, descriptor, "身份和解的工具登记")?;
    if body["original_registration"] != value["registration"]
        || body["reconciliation"] != value["identity_reconciliation"]
    {
        return Err("后继工具登记没有绑定同一身份和解".into());
    }
    Ok(Some(descriptor.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::clone::native::post_process::registration;

    #[test]
    fn registered_successor_only_authorizes_later_identity_attempts() {
        let fixture = tempfile::tempdir().unwrap();
        let backend = fixture.path();
        let run = backend.join(".local-tests/identity-run");
        let evidence = run.join("seed-runtime/identities/first-create-reconciliation");
        fs::create_dir_all(&evidence).unwrap();
        document::write_new(&run.join("manifest.json"), &json!({})).unwrap();
        state::initialize(&run).unwrap();
        let original = run.join("seed-runtime.json");
        let base = json!({"identity_tool":{"path":"old-tool","sha256":"old"},"identity_plan":{"same":true}});
        document::write_new(&original, &base).unwrap();
        let original = common::binding(&original).unwrap();
        let (old, _) = state::begin(&run, "seed-runtime", "identities-apply", &json!({})).unwrap();
        state::finish(&run, old, None, Some("StageError")).unwrap();

        let tool = json!({"path":"successor-tool","sha256":"new"});
        let build = run.join("new-tool-build.json");
        document::write_new(
            &build,
            &json!({"format_version":1,"kind":"devex-seed-identity-successor-build","tool":tool}),
        )
        .unwrap();
        let receipt = evidence.join("first-create-reconciliation-result.json");
        document::write_new(&receipt, &json!({"status":"confirmed"})).unwrap();
        let (number, _) = state::begin(&run, "seed-runtime", MODE, &json!({})).unwrap();
        let result = publish(
            backend,
            &run,
            &original,
            &tool,
            &common::binding(&build).unwrap(),
            &common::binding(&receipt).unwrap(),
        )
        .unwrap();
        assert!(resolve(backend, &run, &result["identity_successor"], None).is_err());
        state::finish(&run, number, Some(&result), None).unwrap();

        let (next, _) = state::begin(&run, "seed-runtime", "identities-apply", &json!({})).unwrap();
        let new = &result["identity_successor"];
        let selected =
            registration::resolve(backend, &run, "seed-runtime", new, Some(next), false).unwrap();
        assert_eq!(selected["identity_tool"], tool);
        assert_eq!(selected["identity_plan"], base["identity_plan"]);
        assert_eq!(
            registration::resolve(backend, &run, "seed-runtime", &original, Some(old), true)
                .unwrap(),
            base,
        );
        assert!(
            registration::resolve(backend, &run, "seed-runtime", new, Some(old), true).is_err()
        );
        assert!(
            registration::resolve(backend, &run, "seed-runtime", &original, Some(next), false)
                .is_err()
        );
    }
}

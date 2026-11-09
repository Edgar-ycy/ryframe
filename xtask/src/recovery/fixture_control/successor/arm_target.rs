//! 后继 arm 目标必须属于关系对应侧的原初始化工作区及已登记存储。
use super::arm::file_binding;
use crate::{
    Result,
    cli::CloneSide,
    recovery::clone::native::{
        bridge::{self, scope::Scope},
        digest, json as document,
        lifecycle::manifest::{self, fixture_storage},
        maintenance,
        resources::{common, storage::registered},
        target_verify::{self, fresh, guard::Inputs, input::request},
    },
    recovery::fixture_control::services::lifecycle::query,
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

#[derive(PartialEq)]
pub(in crate::recovery) struct Target {
    pub(in crate::recovery) registration: Value,
    pub(in crate::recovery) initialized: Value,
    pub(in crate::recovery) files: Value,
    pub(in crate::recovery) environment: Value,
    pub(in crate::recovery) generation: Value,
    pub(in crate::recovery) state: Value,
    pub(in crate::recovery) side: String,
}

pub(super) fn observe(
    backend: &Path,
    workspace: &Path,
    side: &str,
    relation: &Value,
    source: &Value,
    bridges: &[Value],
) -> Result<Target> {
    observe_target(
        backend,
        workspace,
        Selection::Successor {
            relation,
            side: Some(side),
        },
        source,
        bridges,
        false,
    )
}

enum Selection<'a> {
    Successor {
        relation: &'a Value,
        side: Option<&'a str>,
    },
    Ordinary {
        seed: &'a Value,
    },
}

pub(in crate::recovery) fn for_input(
    backend: &Path,
    request: &Value,
    source: &Value,
    successor: bool,
    live: bool,
) -> Result<Target> {
    let path = common::bound_file(backend, &request["target_registration"], "arm 目标登记")?;
    if path
        .file_name()
        .is_none_or(|name| name != "registration.json")
    {
        return Err("arm 目标 registration 文件名无效".into());
    }
    let workspace = path.parent().ok_or("arm 目标缺少工作区")?;
    let selection = if successor {
        Selection::Successor {
            relation: &source["review_successor"],
            side: None,
        }
    } else {
        Selection::Ordinary {
            seed: &source["seed_target"],
        }
    };
    let bridges = document::array(&request["build_bridges"], "arm 构建桥接")?;
    let target = observe_target(
        backend,
        workspace,
        selection,
        &source["request"]["source"],
        bridges,
        live,
    )?;
    if target.registration != request["target_registration"]
        || target.initialized != request["initialized"]
        || target.files != request["target_initialized_files"]
        || target.environment != request["target_environment"]
    {
        return Err("arm 初始化、环境或文件快照不属于同一 fresh 目标登记".into());
    }
    Ok(target)
}

fn observe_target(
    backend: &Path,
    workspace: &Path,
    selection: Selection<'_>,
    source: &Value,
    bridges: &[Value],
    live: bool,
) -> Result<Target> {
    let workspace = common::local_directory(backend, workspace, "arm 目标工作区")?;
    let registration_path = workspace.join("registration.json");
    let initialized_path = workspace.join("target/initialized.json");
    let files_path = workspace.join("initialized-files.json");
    let guard = Inputs::open([
        registration_path.clone(),
        initialized_path.clone(),
        files_path.clone(),
    ])?;
    let registration = file_binding(backend, &registration_path, "arm 目标登记")?;
    let initialized = file_binding(backend, &initialized_path, "arm 目标初始化")?;
    let files = file_binding(backend, &files_path, "arm 目标初始化文件树")?;
    let registered = read_registration(backend, &workspace, &registration)?;
    let environment = registered["environment"].clone();
    verify_environment(backend, &environment)?;
    let observed = target_verify::published::observe_unlocked_history(
        backend,
        &initialized_path,
        |_, target| {
            let prepared = document::read(&workspace.join("target/prepare.json"))?;
            if prepared["request"] != registered["request"] {
                return Err("arm 初始化请求不属于原目标登记".into());
            }
            let side = verify_selection(backend, &target, &registered["request"], selection)?;
            disjoint(source, &target["target"])?;
            let (generation, state) = storage(
                backend,
                &registration,
                &registered["storage_run"],
                &target,
                live,
            )?;
            verify_maintenance(backend, &target, bridges)?;
            common::bound_file(backend, &registered["request"], "arm 原目标请求")?;
            common::bound_file(backend, &environment, "arm 目标环境")?;
            Ok((generation, state, side))
        },
    );
    let checked = guard.check();
    let (generation, state, side) = match (observed, checked) {
        (Ok(value), Ok(())) => value,
        (Err(error), Ok(())) | (Ok(_), Err(error)) => return Err(error),
        (Err(primary), Err(after)) => {
            return Err(format!("{primary}；arm 目标登记后验失败：{after}").into());
        }
    };
    Ok(Target {
        registration,
        initialized,
        files,
        environment,
        generation,
        state,
        side,
    })
}

fn read_registration(backend: &Path, workspace: &Path, registration: &Value) -> Result<Value> {
    let registered = common::read_bound(backend, registration, "arm 目标登记")?;
    document::exact(
        &registered,
        &[
            "format_version",
            "kind",
            "request",
            "environment",
            "storage_run",
            "target_directory",
        ],
        "arm 目标登记",
    )?;
    if registered["format_version"] != 1
        || registered["kind"] != "devex-clone-fresh-target-registration"
        || Path::new(document::text(
            &registered["target_directory"],
            "arm 目标目录",
        )?) != workspace.join("target")
    {
        return Err("arm 目标必须绑定同一 fresh 目标工作区".into());
    }
    Ok(registered)
}

fn verify_selection(
    backend: &Path,
    target: &Value,
    registration: &Value,
    selection: Selection<'_>,
) -> Result<String> {
    let side = document::text(&target["side"], "arm 目标侧")?;
    if !matches!(side, "base" | "candidate") {
        return Err("arm 目标必须是独立的 base 或 candidate 侧".into());
    }
    match selection {
        Selection::Successor {
            relation,
            side: expected,
        } => {
            let expected = match expected {
                Some(expected) => expected,
                None => side,
            };
            verify_selected(target, registration, relation, expected)?;
        }
        Selection::Ordinary { seed } => {
            let seed_review = target_verify::published::request_binding(backend, seed)?;
            let target_review = target_verify::published::request_binding(backend, target)?;
            if seed_review["review"] != target_review["review"] {
                return Err("普通 arm 目标不属于同一 seed 审阅计划".into());
            }
        }
    }
    Ok(side.to_owned())
}

fn verify_environment(backend: &Path, descriptor: &Value) -> Result<()> {
    let environment = common::read_bound(backend, descriptor, "arm 目标环境")?;
    document::exact(&environment, &["environment"], "arm 目标环境")?;
    if document::object(&environment["environment"], "arm 目标环境")?
        .values()
        .any(|value| !value.is_string())
    {
        return Err("arm 目标环境必须是固定文本映射".into());
    }
    Ok(())
}

fn verify_selected(
    target: &Value,
    registration: &Value,
    relation: &Value,
    side: &str,
) -> Result<()> {
    let expected = &relation["requests"][side];
    document::exact(
        expected,
        &["path", "bytes", "sha256", "canonical_sha256"],
        "arm 对应侧请求",
    )?;
    let file =
        json!({"path":expected["path"],"bytes":expected["bytes"],"sha256":expected["sha256"]});
    if target["side"] != side
        || *registration != file
        || digest::plan_hash(target)? != expected["canonical_sha256"]
        || target["review"] != relation["successor_review"]
    {
        return Err("successor arm 目标不是 relationship 对应侧的精确 fresh 请求".into());
    }
    Ok(())
}

fn databases(config: &Value) -> Result<BTreeSet<(&str, String)>> {
    document::array(&config["databases"], "arm 数据库")?
        .iter()
        .map(|value| {
            Ok((
                document::text(&value["server_uuid"], "arm 数据库 UUID")?,
                document::text(&value["database"], "arm 数据库名称")?.to_lowercase(),
            ))
        })
        .collect()
}

fn disjoint(source: &Value, target: &Value) -> Result<()> {
    let source_scope = document::text(&source["scope_id"], "arm 源 scope")?;
    let target_scope = document::text(&target["scope_id"], "arm 目标 scope")?;
    if source_scope == target_scope || !databases(source)?.is_disjoint(&databases(target)?) {
        return Err("arm 源与目标的 scope 或物理数据库重叠".into());
    }
    Ok(())
}

fn storage(
    backend: &Path,
    registration: &Value,
    historical: &Value,
    target: &Value,
    live: bool,
) -> Result<(Value, Value)> {
    if let Some(fixture) = fixture_storage::resolve(backend, registration, None, live)? {
        let _ = target_verify::storage::arm_fixture_projection(
            backend,
            &fixture.run,
            &target["storage"],
        )?;
        if live {
            query::observe_arm_generation(backend, &fixture.run, &fixture.binding["generation"])?;
        }
        fixture.check(backend)?;
        let state = fixture.state_binding().clone();
        return Ok((fixture.binding, state));
    }
    let run = fresh::storage_run(backend, historical, None)?;
    let state = file_binding(backend, &run.join("state.json"), "arm 独立存储账本")?;
    let (selected, cache) = fresh::selection::transition(backend, target, Some(&run), None)?;
    if live && registered::current(backend, &run, CloneSide::Target)? != selected.runtime {
        return Err("arm 实际目标存储不是当前登记代次".into());
    }
    let effective = json!({"path":historical["path"],"manifest":historical["manifest"],
        "storage":selected.runtime,"cache":cache});
    manifest::validate_target_storage(backend, registration, &effective)?;
    common::bound_file(backend, &state, "arm 独立存储账本")?;
    Ok((effective, state))
}

pub(crate) fn verify_maintenance(backend: &Path, target: &Value, bridges: &[Value]) -> Result<()> {
    let (execution, _) = request::execution_backend(
        backend,
        target.get("execution_backend"),
        &request::SystemSourceObserver,
    )?;
    for bridge in bridges {
        document::exact(bridge, &["path", "sha256"], "arm 构建桥接")?;
    }
    let scope = Scope::load(backend, bridges, &[])?;
    let descriptor = &target["maintenance_build"];
    let path = common::bound_file(&execution, descriptor, "arm 维护构建")?;
    let receipt = maintenance::verify_evidence(&execution, &path, None)?;
    let jobs = bridge::build_jobs(&execution)?;
    maintenance::require_build_jobs(&receipt, jobs)?;
    scope.maintenance(&execution, &path, &receipt)?;
    if bridge::build_jobs(&execution)? != jobs {
        return Err("arm 维护构建并发度在核验期间变化".into());
    }
    common::bound_file(&execution, descriptor, "arm 维护构建")?;
    scope.assert_unchanged()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_target_requires_the_precise_relationship_side_and_physical_isolation() {
        let review =
            json!({"path":"bound-review","bytes":7,"sha256":"raw","canonical_sha256":"model"});
        let target = json!({"side":"candidate","review":review,
            "target":{"scope_id":"target-side","databases":[{"server_uuid":"primary","database":"data"}]}});
        let raw = json!({"path":"bound-request","bytes":17,"sha256":"original"});
        let mut descriptor = raw.clone();
        descriptor["canonical_sha256"] = json!(digest::plan_hash(&target).unwrap());
        let relation = json!({"successor_review":review,"requests":{"candidate":descriptor}});
        verify_selected(&target, &raw, &relation, "candidate").unwrap();
        let mut wrong = raw.clone();
        wrong["sha256"] = json!("other");
        assert!(verify_selected(&target, &wrong, &relation, "candidate").is_err());
        let mut wrong = target.clone();
        wrong["side"] = json!("base");
        assert!(verify_selected(&wrong, &raw, &relation, "candidate").is_err());
        wrong = target.clone();
        wrong["review"]["sha256"] = json!("other");
        assert!(verify_selected(&wrong, &raw, &relation, "candidate").is_err());

        let mut source = json!({"scope_id":"seed-side","databases":[{"server_uuid":"secondary","database":"DATA"}]});
        disjoint(&source, &target["target"]).unwrap();
        source["databases"][0]["server_uuid"] = json!("primary");
        assert!(disjoint(&source, &target["target"]).is_err());
        source["databases"][0]["server_uuid"] = json!("secondary");
        source["scope_id"] = json!("target-side");
        assert!(disjoint(&source, &target["target"]).is_err());
    }
}

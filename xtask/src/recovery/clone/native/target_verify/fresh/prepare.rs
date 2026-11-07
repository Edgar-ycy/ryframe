//! prepare 与显式续作的本地发布状态机；任何未知写入均交给既有严格证据模型拒绝。
use super::super::super::{
    bridge::scope::Scope, export::transport::SystemTransport, json as document, resources::common,
};
use super::super::{cli, creation_plan, prepare_state, resume_baseline, tree};
use super::{
    receipts,
    registration::{Registration, Workspace, combine},
    resources::Resources,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(crate) fn prepare(
    backend: &Path,
    workspace: &Path,
    request: &Path,
    environment: &Path,
    storage: &Path,
) -> Result<Value> {
    let scope = Scope::load(backend, &[], &[])?;
    let result = (|| {
        let workspace = local(backend, workspace, LocalTestPathKind::NewDirectory)?;
        if !workspace.parent().is_some_and(Path::is_dir) {
            return Err("fresh 工作区必须位于已有父目录".into());
        }
        let request = local(backend, request, LocalTestPathKind::ExistingFile)?;
        let environment = local(backend, environment, LocalTestPathKind::ExistingFile)?;
        let storage = local(backend, storage, LocalTestPathKind::ExistingDirectory)?;
        cli::execute(
            backend,
            &workspace,
            "cli-disjoint",
            &json!({"paths":[workspace,storage]}),
        )?;
        let request = common::binding(&request)?;
        common::bound_file(backend, &request, "fresh 固定请求")?;
        let environment = common::binding(&environment)?;
        let private = super::super::binding::private_environment(backend, &environment)?;
        let storage = json!({"path":storage,"manifest":common::binding(&storage.join("manifest.json"))?,"state":common::binding(&storage.join("state.json"))?});
        super::storage::load(backend, &storage)?;
        fs::create_dir(&workspace)?;
        let control = Workspace::claim(backend, &workspace)?;
        let result = (|| {
            common::bound_file(backend, &request, "fresh 固定请求")?;
            if super::super::binding::private_environment(backend, &environment)? != private {
                return Err("fresh 私有环境在取得锁前变化".into());
            }
            super::storage::load(backend, &storage)?;
            document::write_new(
                &workspace.join("registration.json"),
                &json!({"format_version":1,
                "kind":"devex-clone-fresh-target-registration","request":request,"environment":environment,
                "storage_run":storage,"target_directory":workspace.join("target")}),
            )?;
            run(&Registration::load(backend, &workspace)?, &scope, false)
        })();
        control.finish(result)
    })();
    combine(result, scope.assert_unchanged())
}

pub(crate) fn resume(backend: &Path, workspace: &Path) -> Result<Value> {
    let scope = Scope::load(backend, &[], &[])?;
    let workspace = local(backend, workspace, LocalTestPathKind::ExistingDirectory)?;
    let result = (|| {
        let registered = Registration::load(backend, &workspace)?;
        resumable(&registered)?;
        let mut runtime =
            crate::recovery::fixture_control::services::lifecycle::readonly::NativeRuntime::new(
                backend,
            );
        if !runtime.observe_controller(&registered.storage)?.is_null() {
            return Err("fresh 控制锁尚未收尾；先核对 status，再使用原 recover 入口".into());
        }
        let control = Workspace::claim(backend, &workspace)?;
        let result = (|| {
            registered.check(true)?;
            resumable(&registered)?;
            run(&registered, &scope, true)
        })();
        control.finish(result)
    })();
    combine(result, scope.assert_unchanged())
}

fn local(backend: &Path, path: &Path, kind: LocalTestPathKind) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        backend.join(path)
    };
    Ok(validate_local_test_path(&path, backend, kind)?)
}

fn resumable(registration: &Registration) -> Result<Value> {
    if registration
        .workspace
        .join("prepared-files.json")
        .try_exists()?
    {
        let descriptor = common::binding(&registration.workspace.join("prepared-files.json"))?;
        resume_baseline::prepared(
            &registration.backend,
            &registration.target,
            &json!({
                "descriptor":descriptor,"registration":registration.binding,"predecessor":null
            }),
            &registration.value["request"],
        )?;
        return Err("fresh 目标 prepare 已完整发布，不能重复续作".into());
    }
    let state = prepare_state::state(
        &registration.backend,
        &registration.target,
        &registration.value["request"],
    )?;
    if state["resumable"] != true {
        return Err("fresh prepare 不能续作；核对 status 与原始证据，禁止自动清理".into());
    }
    Ok(state)
}

fn run(registration: &Registration, scope: &Scope, resume: bool) -> Result<Value> {
    let result = registration.execute(|storage| {
        let state = if resume {
            Some(resumable(registration)?)
        } else {
            None
        };
        let environment = document::object(&registration.environment, "fresh 私有环境")?
            .iter()
            .map(|(key, value)| {
                Ok((
                    key.to_ascii_uppercase(),
                    document::text(value, "私有环境值")?.to_owned(),
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let request = common::read_bound(
            &registration.backend,
            &registration.value["request"],
            "fresh 请求",
        )?;
        let transport = SystemTransport;
        let resources = Resources {
            inputs: super::super::storage::StorageInputs {
                backend: &registration.backend,
                request: &request,
                environment: &environment,
                observation_dir: &registration.target,
            },
            transport: &transport,
            scope,
            run: storage,
        };
        leaf(registration, &resources, state.as_ref())
    })?;
    let files = publish(registration, &result)?;
    let mut value = json!({"status":result["status"],"registration":registration.binding,
        "target":common::binding(&registration.target.join("prepare.json"))?,"files":files,"restore_qualified":false});
    if resume {
        value["resumed"] = json!(true);
    }
    Ok(value)
}

fn leaf(
    registration: &Registration,
    resources: &Resources<'_, impl super::super::super::export::transport::Transport>,
    state: Option<&Value>,
) -> Result<Value> {
    if state.is_none() {
        validate_local_test_path(
            &registration.target,
            &registration.backend,
            LocalTestPathKind::NewDirectory,
        )?;
        fs::create_dir(&registration.target)?;
        let mut stage = "request";
        let result = collect(registration, resources, false, &mut stage);
        return match result {
            Ok(value) => Ok(value),
            Err(error) => combine(
                Err(error),
                document::write_new(
                    &registration.target.join("failure.json"),
                    &receipts::failure(stage),
                ),
            ),
        };
    }
    if !registration.target.try_exists()? {
        fs::create_dir(&registration.target)?;
    }
    let state = state.ok_or("prepare 续作缺少已验证前像")?;
    super::resume_prepare::execute(registration, state, |prepared| {
        collect(registration, resources, prepared, &mut "prepare_binding")
    })
}

fn collect(
    registration: &Registration,
    resources: &Resources<'_, impl super::super::super::export::transport::Transport>,
    prepared: bool,
    stage: &mut &str,
) -> Result<Value> {
    let request = common::read_bound(
        &registration.backend,
        &registration.value["request"],
        "fresh 固定请求",
    )?;
    let copy = registration.target.join("request.json");
    if !copy.try_exists()? {
        document::write_new(&copy, &request)?;
    }
    if document::read(&copy)? != request {
        return Err("fresh 中断请求不同于固定 registration".into());
    }
    *stage = "prepare_binding";
    let generation = resources.generation()?;
    *stage = "prepare_absence";
    let absence = resources.absence(&generation["storage"]["redis"])?;
    if resources.generation()? != generation {
        return Err("fresh 只读观察期间完整代次变化".into());
    }
    common::bound_file(
        &registration.backend,
        &registration.value["request"],
        "fresh 固定请求",
    )?;
    if document::read(&copy)? != request {
        return Err("fresh 请求副本在观察期间变化".into());
    }
    prepare_document(registration, &request, generation, absence, prepared)
}

fn prepare_document(
    registration: &Registration,
    request: &Value,
    generation: Value,
    absence: Value,
    prepared: bool,
) -> Result<Value> {
    let observed = if prepared {
        document::read(&registration.target.join("prepare.json"))?
    } else {
        Value::Null
    };
    let at = if prepared {
        observed["prepared_at"].clone()
    } else {
        json!(chrono::Utc::now().to_rfc3339())
    };
    let mut args = json!({"at":at,"request_descriptor":registration.value["request"],"generation":generation,"absence":absence});
    if prepared {
        let mut historical = observed["generation"].clone();
        let mut actual = args["generation"].clone();
        historical
            .as_object_mut()
            .ok_or("历史 generation 无效")?
            .remove("storage");
        actual
            .as_object_mut()
            .ok_or("实际 generation 无效")?
            .remove("storage");
        if historical != actual {
            return Err("fresh prepared 的配置、来源或维护工具变化".into());
        }
        super::super::resume_baseline::document(
            &registration.backend,
            &registration.target,
            &observed,
            &registration.value["request"],
            request,
        )?;
        args["generation"] = observed["generation"].clone();
        args["observed"] = observed.clone();
        creation_plan::execute(
            &registration.backend,
            &registration.target,
            "prepare-current",
            &args,
        )?;
        Ok(observed)
    } else {
        let value = creation_plan::execute(
            &registration.backend,
            &registration.target,
            "prepare-document",
            &args,
        )?;
        document::write_new(&registration.target.join("prepare.json"), &value)?;
        Ok(value)
    }
}

fn publish(registration: &Registration, result: &Value) -> Result<Value> {
    registration.check(true)?;
    let files = tree::snapshot(&registration.backend, &registration.target, false)?;
    let request = common::read_bound(
        &registration.backend,
        &registration.value["request"],
        "fresh 请求",
    )?;
    resume_baseline::validate(
        &registration.backend,
        &registration.target,
        &files,
        &registration.value["request"],
        &request,
    )?;
    if tree::snapshot(&registration.backend, &registration.target, false)? != files
        || document::read(&registration.target.join("prepare.json"))? != *result
    {
        return Err("fresh prepare 快照发布前树或结果变化".into());
    }
    let path = registration.workspace.join("prepared-files.json");
    document::write_new(
        &path,
        &json!({"format_version":1,"kind":"devex-clone-fresh-target-prepared-files",
        "registration":registration.binding,"predecessor":null,"files":files}),
    )?;
    let descriptor = common::binding(&path)?;
    resume_baseline::prepared(
        &registration.backend,
        &registration.target,
        &json!({"descriptor":descriptor,
        "registration":registration.binding,"predecessor":null}),
        &registration.value["request"],
    )?;
    registration.check(true)?;
    Ok(descriptor)
}

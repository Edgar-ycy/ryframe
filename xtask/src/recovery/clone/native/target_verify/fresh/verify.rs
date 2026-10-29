//! 复制前完整初像复验；正式工作区和直接目标共用同一只读资源窗口。
mod inputs;
pub(crate) mod window;

use super::super::super::{
    bridge::scope::Scope, export::transport::SystemTransport, json as document, resources::common,
};
use super::super::{guard::Generation, published, storage::StorageInputs};
use super::{
    environment, initialize, receipts,
    registration::{Registration, Workspace, combine},
    resources::Resources,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

pub(crate) fn execute(backend: &Path, workspace: &Path, observation: &Path) -> Result<Value> {
    let scope = Scope::load(backend, &[], &[])?;
    let registration = Registration::load(backend, workspace)?;
    let control = Workspace::claim(backend, workspace)?;
    let released = super::snapshot::verify(
        backend,
        workspace,
        &registration.target,
        &registration.binding,
        "initialized",
    )?;
    let published_inputs =
        super::super::guard::Inputs::open([workspace.join("initialized-files.json")])?;
    disjoint(observation, workspace)?;
    disjoint(observation, &registration.storage)?;
    let descriptor = common::binding(&registration.target.join("initialized.json"))?;
    prepare_observation(
        backend,
        &descriptor,
        observation,
        Some(&registration.storage),
    )?;
    let result = registration
        .execute(|active| registered(&registration, observation, &descriptor, &scope, active));
    let result = combine(control.finish(result), scope.assert_unchanged());
    let result = result.and_then(|(value, protected, temporary, parent_inputs)| {
        published_inputs.check()?;
        parent_inputs.check()?;
        protected.check()?;
        common::directory_identity(backend, &temporary)?;
        initialize::prepared(&registration)?;
        if super::snapshot::verify(
            backend,
            workspace,
            &registration.target,
            &registration.binding,
            "initialized",
        )? != released
        {
            return Err("fresh verify 父收尾后的初始化释放快照变化".into());
        }
        Ok(value)
    });
    finish(observation, result).and_then(|value| Ok(json!({"status":value["status"],
        "registration":registration.binding,"observation":common::binding(&observation.join("verify.json"))?,
        "restore_qualified":false})))
}

fn registered(
    registration: &Registration,
    observation: &Path,
    descriptor: &Value,
    scope: &Scope,
    active: Option<&Path>,
) -> Result<(
    Value,
    super::super::guard::Inputs,
    Value,
    super::super::guard::Inputs,
)> {
    let backend = &registration.backend;
    initialize::prepared(registration)?;
    let filename = registration.target.join("initialized.json");
    let original = published::unlocked_history(backend, &filename)?;
    let prepared = document::read(&registration.target.join("prepare.json"))?;
    if prepared["request"] != registration.value["request"] {
        return Err("verify 请求不同于固定 registration".into());
    }
    let environment = environment::registered(registration, &original.1)?;
    let protected = initialize::inputs::protect(registration, &original.1, &environment)?;
    let (value, inputs, temporary) = run(
        backend,
        descriptor,
        observation,
        active,
        &environment,
        scope,
        None,
    )?;
    protected.check()?;
    if published::unlocked_history(backend, &filename)? != original {
        return Err("fresh 目标 verify 期间初始化文件快照变化".into());
    }
    Ok((value, inputs, temporary, protected))
}

pub(crate) fn binding(backend: &Path, args: &Value) -> Result<Value> {
    document::exact(
        args,
        &[
            "initialized",
            "observation",
            "storage_run",
            "temporary_directory",
            "app_env",
            "write",
        ],
        "fresh 目标复验请求",
    )?;
    if args["write"] != true || args["app_env"] != "test" {
        return Err("fresh 目标复验需要本地写授权及明确 app_env=test".into());
    }
    let observation = Path::new(document::text(&args["observation"], "fresh 观察目录")?);
    let active = if args["storage_run"].is_null() {
        None
    } else {
        Some(Path::new(document::text(
            &args["storage_run"],
            "fresh 存储运行",
        )?))
    };
    let filename = common::bound_file(backend, &args["initialized"], "fresh 初始化绑定")?;
    let prepared = document::read(
        &filename
            .parent()
            .ok_or("初始化路径缺少目标目录")?
            .join("prepare.json"),
    )?;
    let request = common::read_bound(backend, &prepared["request"], "fresh 固定请求")?;
    let temporary = Path::new(document::text(
        &args["temporary_directory"],
        "fresh 私有临时目录",
    )?);
    let temporary_identity = environment::temporary_identity(backend, temporary)?;
    let environment = environment::test(backend, &request, observation, temporary);
    let scope = Scope::load(backend, &[], &[])?;
    prepare_observation(backend, &args["initialized"], observation, active)?;
    let result = run(
        backend,
        &args["initialized"],
        observation,
        active,
        &environment,
        &scope,
        Some(&temporary_identity),
    );
    let result =
        combine(result, scope.assert_unchanged()).and_then(|(value, protected, temporary)| {
            protected.check()?;
            common::directory_identity(backend, &temporary)?;
            Ok(value)
        });
    finish(observation, result)
}

fn run(
    backend: &Path,
    descriptor: &Value,
    observation: &Path,
    active: Option<&Path>,
    environment: &BTreeMap<String, String>,
    scope: &Scope,
    temporary_identity: Option<&Value>,
) -> Result<(Value, super::super::guard::Inputs, Value)> {
    let filename = common::bound_file(backend, descriptor, "fresh 初始化绑定")?;
    let target = filename.parent().ok_or("初始化路径缺少目标目录")?;
    if filename != target.join("initialized.json")
        || environment.get("APP_ENV").map(String::as_str) != Some("test")
    {
        return Err("fresh 目标复验必须绑定 initialized.json 及 APP_ENV=test".into());
    }
    let temporary = environment.get("TEMP").ok_or("fresh 复验缺少已绑定 TEMP")?;
    if environment.get("TMP") != Some(temporary) {
        return Err("fresh 复验 TEMP/TMP 必须保持同一已绑定私有目录".into());
    }
    let actual_temporary = environment::temporary_identity(backend, Path::new(temporary))?;
    if temporary_identity.is_some_and(|expected| expected != &actual_temporary) {
        return Err("fresh 私有临时目录在进入观察前发生身份变化；保留现场".into());
    }
    let temporary = actual_temporary;
    let before = super::super::tree::snapshot(backend, target, false)?;
    let owner = Generation::acquire(backend, target)?;
    let result = (|| {
        let (initialized, request) = published::evidence_history(backend, &filename, Some(&owner))?;
        published::request_binding(backend, &request)?;
        let resources = Resources {
            inputs: StorageInputs {
                backend,
                request: &request,
                environment,
                observation_dir: observation,
            },
            transport: &SystemTransport,
            scope,
            run: active,
        };
        let observer = super::super::storage::SystemObserver::new(&SystemTransport);
        let generation = || resources.generation_owned(Some(&owner));
        window::Window {
            inputs: StorageInputs {
                backend,
                request: &request,
                environment,
                observation_dir: observation,
            },
            target,
            descriptor,
            initialized: &initialized,
            owner: &owner,
            transport: &SystemTransport,
            observer: &observer,
            generation: &generation,
            temporary: &temporary,
        }
        .observe()
    })();
    let result = owner.finish(result);
    let check = (|| {
        if super::super::tree::snapshot(backend, target, false)? != before {
            return Err("目标释放后的完整初始化树变化".into());
        }
        Ok(())
    })();
    combine(result, check).map(|(value, protected)| (value, protected, temporary))
}

fn prepare_observation(
    backend: &Path,
    descriptor: &Value,
    observation: &Path,
    active: Option<&Path>,
) -> Result<()> {
    let filename = common::bound_file(backend, descriptor, "fresh 初始化绑定")?;
    let target = filename.parent().ok_or("初始化路径缺少目标目录")?;
    if filename != target.join("initialized.json") {
        return Err("fresh 复验必须绑定固定 initialized.json".into());
    }
    validate_local_test_path(observation, backend, LocalTestPathKind::NewDirectory)?;
    disjoint(observation, target)?;
    if let Some(run) = active {
        validate_local_test_path(run, backend, LocalTestPathKind::ExistingDirectory)?;
        disjoint(observation, run)?;
    }
    std::fs::create_dir(observation)?;
    Ok(())
}

pub(super) fn disjoint(left: &Path, right: &Path) -> Result<()> {
    // 受管路径校验已拒绝链接；Windows 路径比较必须同时覆盖大小写等价。
    let left = left
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_owned();
    let right = right
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_owned();
    let (left, right) = if cfg!(windows) {
        (left.to_lowercase(), right.to_lowercase())
    } else {
        (left, right)
    };
    if left == right || left.starts_with(&(right.clone() + "/")) || right.starts_with(&(left + "/"))
    {
        return Err("fresh 观察目录必须与目标、工作区及存储运行互不包含".into());
    }
    Ok(())
}

pub(crate) fn finish(observation: &Path, result: Result<Value>) -> Result<Value> {
    let result = result.and_then(|value| {
        receipts::write(&observation.join("verify.json"), &value)?;
        Ok(value)
    });
    if let Err(error) = &result
        && observation.is_dir()
        && !observation.join("failure.json").try_exists()?
    {
        let mut body = receipts::failure("verify_existing");
        body["error_type"] = json!(super::failure::category(error.as_ref()));
        body["reason"] = json!(
            "只读目标复验未完成；核对固定输入与完整观察，保留原初始化现场，禁止自动重放或清理"
        );
        return combine(
            result,
            receipts::write(&observation.join("failure.json"), &body),
        );
    }
    result
}

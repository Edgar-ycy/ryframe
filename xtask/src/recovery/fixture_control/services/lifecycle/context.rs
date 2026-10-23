//! 服务上下文只读装载；凭据只返回私有调用方，不写日志或公开状态。
use super::*;
use crate::local_test_path::{LocalTestPathKind, validate_local_test_path};

pub(super) fn document(backend: &Path, selected: &Path) -> Result<(PathBuf, Value)> {
    let path = if selected.is_absolute() {
        selected.to_path_buf()
    } else {
        backend.join(selected)
    };
    validate_local_test_path(&path, backend, LocalTestPathKind::ExistingFile)?;
    let before = binding(&path)?;
    let value = read(&path)?;
    check(binding(&path)? == before, "夹具服务输入在读取期间变化")?;
    Ok((path, value))
}

pub(super) fn service_run(review: &Value) -> Result<PathBuf> {
    let root = PathBuf::from(text(review, "future_root")?);
    let backend = PathBuf::from(text(&review["scopes"]["seed"], "backend_dir")?);
    check(
        root.is_absolute()
            && backend.is_absolute()
            && root.parent()
                == Some(
                    backend
                        .join(".local-tests")
                        .join("reference-fixture")
                        .as_path(),
                )
            && !root
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir)),
        "审阅计划 future_root 必须是业务 crate 参考夹具目录的直接子目录",
    )?;
    Ok(root.join("service-run"))
}

pub(super) fn environment(
    runtime: &mut dyn Runtime,
    backend: &Path,
    review_file: &Path,
    review: &Value,
    selected: &Path,
) -> Result<(PathBuf, Value, Value)> {
    let (path, bootstrap) = document(backend, selected)?;
    check(
        bootstrap["kind"] == "reference-fixture-environment"
            && bootstrap["status"] == "prepared"
            && bootstrap["services_started"] == false
            && bootstrap["remote_writes"] == 0,
        "夹具环境尚未准备完成或已经发生服务副作用",
    )?;
    let original = &bootstrap["plan"]["review"];
    let descriptor = descriptor(original);
    let current = binding(review_file)?;
    if descriptor != current {
        check(
            continued_review(runtime, backend, &descriptor, review)?,
            "夹具环境未绑定当前审阅收据",
        )?;
    }
    let environment = read(
        &path
            .parent()
            .ok_or("环境收据没有父目录")?
            .join("environment.json"),
    )?;
    let values = &environment["environment"];
    check(
        values
            .as_object()
            .is_some_and(|items| items.values().all(Value::is_string)),
        "夹具私有环境无效",
    )?;
    Ok((path, bootstrap, values.clone()))
}

fn continued_review(
    runtime: &mut dyn Runtime,
    backend: &Path,
    original: &Value,
    review: &Value,
) -> Result<bool> {
    if review["preflight"]["supersedes"] != *original || !original["path"].is_string() {
        return Ok(false);
    }
    let (path, previous) = match document(backend, Path::new(text(original, "path")?)) {
        Ok(value) => value,
        Err(_) => return Ok(false),
    };
    if runtime
        .call(
            "review-successor",
            json!({"previous":previous,"review":review}),
        )
        .is_err()
    {
        return Ok(false);
    }
    Ok(binding(&path)? == *original)
}

fn descriptor(value: &Value) -> Value {
    json!({"path":value["path"],"bytes":value["bytes"],"sha256":value["sha256"]})
}

pub(super) fn load(
    runtime: &mut dyn Runtime,
    backend: &Path,
    review_path: &Path,
    bootstrap_path: &Path,
) -> Result<Value> {
    let (review_file, review) = document(backend, review_path)?;
    let (bootstrap_file, bootstrap, private) =
        environment(runtime, backend, &review_file, &review, bootstrap_path)?;
    let run = service_run(&review)?;
    validate_local_test_path(&run, backend, LocalTestPathKind::ExistingDirectory)?;
    let (manifest_file, manifest) = document(backend, &run.join("manifest.json"))?;
    history::validate_manifest(&manifest)?;
    check(
        (manifest["review"] == binding(&review_file)?
            || manifest["review"] == descriptor(&bootstrap["plan"]["review"]))
            && manifest["bootstrap"] == binding(&bootstrap_file)?
            && manifest["execution_backend"] == bootstrap["execution_backend"]
            && manifest["scope_id"] == review["services"]["rustfs"]["scope_id"],
        "夹具服务生命周期未绑定原始审阅、私有环境与执行目录",
    )?;
    let before = binding(&run.join("state.json"))?;
    let state = state::load(&run, true)?;
    let history = history::validate(&run, &state, false, false)?;
    let value = json!({"backend":backend,"run":run,"manifest":manifest,"state":state,"history":history,
        "sources":{"review":binding(&review_file)?,"bootstrap":binding(&bootstrap_file)?,"manifest":binding(&manifest_file)?,
        "state_before":before,"environment":binding(&bootstrap_file.parent().ok_or("环境收据没有父目录")?.join("environment.json"))?},
        "private":private,"documents":{"review":review,"bootstrap":bootstrap,"manifest":manifest}});
    Context::load(backend, value.clone())?;
    Ok(value)
}

pub(super) fn guarded(backend: &Path, value: &Value, running: Option<&Value>) -> Result<Context> {
    let run = PathBuf::from(text(value, "run")?);
    validate_local_test_path(&run, backend, LocalTestPathKind::ExistingDirectory)?;
    check(
        Path::new(text(value, "backend")?) == backend,
        "服务上下文属于其他后端",
    )?;
    let context = Context {
        backend: backend.to_path_buf(),
        run,
        value: value.clone(),
    };
    context.guard(running)?;
    Ok(context)
}

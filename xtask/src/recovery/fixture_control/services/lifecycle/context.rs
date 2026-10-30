//! 服务上下文只读装载；凭据只返回私有调用方，不写日志或公开状态。
use super::*;
use crate::local_test_path::{LocalTestPathKind, same_local_test_file, validate_local_test_path};

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

fn recorded_path(backend: &Path, selected: &Path, recorded: &Value) -> Result<Option<PathBuf>> {
    let expected = Path::new(text(recorded, "path")?);
    if !same_local_test_file(backend, selected, expected)? {
        return Ok(None);
    }
    let (path, _) = document(backend, expected)?;
    check(
        binding(&path)? == descriptor(recorded),
        "登记文件内容或原路径绑定变化",
    )?;
    Ok(Some(path))
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
    let registered = recorded_path(backend, review_file, &descriptor)?;
    let review_file = registered.as_deref().unwrap_or(review_file);
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

fn review_path(
    backend: &Path,
    selected: &Path,
    bootstrap: &Value,
    manifest: &Value,
    state: &Value,
) -> Result<(PathBuf, Value)> {
    for recorded in [&bootstrap["plan"]["review"], &manifest["review"]] {
        if let Some(path) = recorded_path(backend, selected, recorded)? {
            return Ok((path, descriptor(recorded)));
        }
    }
    for attempt in state::attempts(state)?
        .iter()
        .rev()
        .filter(|attempt| attempt["sources"]["review"].is_object())
    {
        let recorded = &attempt["sources"]["review"];
        // 仅从已验证历史保留同一位置的字面拼写，不将其他历史输入授予当前作用域。
        json_io::exact(recorded, &["path", "bytes", "sha256"], "已登记服务审阅")?;
        if let Some(path) = recorded_path(backend, selected, recorded)? {
            return Ok((path, descriptor(recorded)));
        }
    }
    // 真正的新审阅仍须由 environment 的原 successor 合同授权。
    let path = super::super::super::portable_path(&std::fs::canonicalize(selected)?);
    let sampled = binding(&path)?;
    Ok((path, sampled))
}

pub(super) fn load(
    runtime: &mut dyn Runtime,
    backend: &Path,
    review_path: &Path,
    bootstrap_path: &Path,
) -> Result<Value> {
    let (selected_review, review) = document(backend, review_path)?;
    let run = service_run(&review)?;
    validate_local_test_path(&run, backend, LocalTestPathKind::ExistingDirectory)?;
    let (manifest_file, manifest) = document(backend, &run.join("manifest.json"))?;
    history::validate_manifest(&manifest)?;
    for key in ["review", "bootstrap"] {
        json_io::exact(
            &manifest[key],
            &["path", "bytes", "sha256"],
            "服务清单文件绑定",
        )?;
    }
    let bootstrap_file = recorded_path(backend, bootstrap_path, &manifest["bootstrap"])?
        .ok_or_else(|| model::error("夹具环境输入不是原清单登记的同一文件"))?;
    let (_, sampled_bootstrap) = document(backend, &bootstrap_file)?;
    let before = binding(&run.join("state.json"))?;
    let state = state::load(&run, true)?;
    let history = history::validate(&run, &state, false, false)?;
    let (review_file, review_binding) = self::review_path(
        backend,
        &selected_review,
        &sampled_bootstrap,
        &manifest,
        &state,
    )?;
    check(
        read(&review_file)? == review,
        "服务审阅在选择登记路径期间变化",
    )?;
    let (_, bootstrap, private) =
        environment(runtime, backend, &review_file, &review, &bootstrap_file)?;
    let manifest_review = recorded_path(backend, &review_file, &manifest["review"])?;
    let original_review = recorded_path(
        backend,
        Path::new(text(&bootstrap["plan"]["review"], "path")?),
        &manifest["review"],
    )?;
    check(
        (manifest_review.is_some() || original_review.is_some())
            && manifest["bootstrap"] == binding(&bootstrap_file)?
            && manifest["execution_backend"] == bootstrap["execution_backend"]
            && manifest["scope_id"] == review["services"]["rustfs"]["scope_id"],
        "夹具服务生命周期未绑定原始审阅、私有环境与执行目录",
    )?;
    let value = json!({"backend":backend,"run":run,"manifest":manifest,"state":state,"history":history,
        "sources":{"review":review_binding,"bootstrap":manifest["bootstrap"],"manifest":binding(&manifest_file)?,
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

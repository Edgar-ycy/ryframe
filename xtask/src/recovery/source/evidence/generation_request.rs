//! 当前来源代次请求的原生构造器；预览只读，发布只创建新文件。
use super::{binding, running};
use crate::{
    Result,
    cli::FixtureSuccessorGeneration,
    recovery::clone::native::{
        bridge, cache, digest, json as document, lifecycle::state, resources::common,
        seed::generation::coordinator,
    },
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[path = "generation_request/configuration.rs"]
pub(crate) mod configuration;

pub(crate) fn run(backend: &Path, options: &FixtureSuccessorGeneration) -> Result<Value> {
    if let Some(output) = &options.output {
        validate_output(backend, output)?;
    }
    validate_options(backend, options)?;
    let arguments = arguments(backend, options)?;
    let request = build(&arguments)?;
    if let Some(output) = &options.output {
        publish(backend, output, &arguments, &request)?;
    }
    Ok(
        json!({"status":if options.output.is_some() {"source_generation_request_published"} else {"source_generation_request_planned"},
        "request":request,"request_sha256":digest::plan_hash(&request)?,"remote_writes":0}),
    )
}

pub(crate) fn validate_options(backend: &Path, options: &FixtureSuccessorGeneration) -> Result<()> {
    use crate::local_test_path::{LocalTestPathKind, validate_absolute_path};
    for path in [&options.source_backend, &options.config_dir] {
        validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)?;
    }
    if let Some(path) = &options.product_backend {
        validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)?;
    }
    for path in [
        &options.successor,
        &options.backend_build,
        &options.source_environment,
    ] {
        common::common_file(backend, path, "来源请求输入")?;
    }
    common::common_file(
        &options.source_backend,
        &options.maintenance_build,
        "来源维护构建",
    )?;
    if let Some(path) = &options.execution_source {
        common::common_file(backend, path, "执行源码发布收据")?;
    }
    Ok(())
}

fn validate_output(backend: &Path, output: &Path) -> Result<()> {
    crate::local_test_path::validate_local_test_path(
        output,
        backend,
        crate::local_test_path::LocalTestPathKind::OutputFile,
    )?;
    crate::local_test_path::validate_absolute_path(
        output.parent().ok_or("请求输出缺少父目录")?,
        crate::local_test_path::LocalTestPathKind::ExistingDirectory,
    )?;
    Ok(())
}

fn arguments(backend: &Path, options: &FixtureSuccessorGeneration) -> Result<Value> {
    Ok(
        json!({"backend":backend,"review_successor":common::binding(&options.successor)?,
        "execution_backend":options.source_backend,"expected_backend_sha":options.expected_head,
        "backend_build":common::binding(&options.backend_build)?,"maintenance_build":common::binding(&options.maintenance_build)?,
        "source_environment":common::binding(&options.source_environment)?,"id":options.id,
        "adapter_contract":options.adapter_contract,"product_backend":options.product_backend,
        "configuration":configuration::capture(backend, &options.config_dir)?,
        "execution_source":options.execution_source.as_ref().map(|path|common::binding(path)).transpose()?}),
    )
}

pub(crate) fn request(args: &Value) -> Result<Value> {
    document::exact(args, &["backend", "request"], "来源请求重算参数")?;
    let backend = Path::new(document::text(&args["backend"], "协调后端")?);
    let request = &args["request"];
    coordinator::current_request(request)?;
    let mut arguments = request.clone();
    let object = arguments.as_object_mut().ok_or("来源请求必须为对象")?;
    for field in [
        "format_version",
        "kind",
        "source_registration",
        "source_rebind",
        "current_storage",
    ] {
        object.remove(field);
    }
    object.insert("backend".into(), json!(backend));
    let rebuilt = build(&arguments)?;
    if rebuilt != *request {
        return Err("来源请求与原生构造器重算的完整绑定不同；不改写原请求或自动重放".into());
    }
    Ok(rebuilt)
}

fn build(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &[
            "backend",
            "review_successor",
            "execution_backend",
            "expected_backend_sha",
            "backend_build",
            "maintenance_build",
            "source_environment",
            "id",
            "adapter_contract",
            "product_backend",
            "configuration",
            "execution_source",
        ],
        "当前来源构造参数",
    )?;
    let backend = Path::new(document::text(&args["backend"], "协调后端")?);
    let initial = cache::binding::source::load(backend, &args["review_successor"])?;
    let run = PathBuf::from(document::text(&initial["directory"], "来源账本")?);
    let saved = state::load(&run, true)?;
    let source = running::history::predecessor(backend, &run, &saved)?;
    if source["review_successor_binding"] != args["review_successor"]
        || source["source_rebind"].is_null()
    {
        return Err("当前请求必须继承同一successor和已发布存储重绑定".into());
    }
    let published = coordinator::dispatch(
        &json!({"action":"select-published","input":{"attempts":saved["attempts"]}}),
    )?;
    if published.get("rejected").is_some() || !published["record"].is_null() {
        return Err("当前请求必须在已发布重绑定后、首次来源代次发布前构造；不改写历史".into());
    }
    let mut value = args.clone();
    value
        .as_object_mut()
        .ok_or("来源构造参数必须为对象")?
        .remove("backend");
    value["format_version"] = json!(2);
    value["kind"] = json!("devex-clone-seed-source-generation");
    value["source_registration"] = source["review_successor"]["source_result"].clone();
    value["source_rebind"] = source["source_rebind"].clone();
    value["current_storage"] = source["storage"]["storage"].clone();
    coordinator::current_request(&value)?;
    let configuration = configuration::verify(backend, &value)?;
    configuration::execution_source(backend, &value)?;
    let build = running::builds::verify(backend, &value)?;
    let execution = Path::new(document::text(&value["execution_backend"], "来源执行根")?);
    let current = bridge::build_context::current(execution)?;
    if !matches_build_context(&build["build"], &current, &value, &source) {
        return Err("当前来源构建与实际工具链或构建环境不符".into());
    }
    let physical = binding::physical(&json!({"backend":execution,"evidence_root":backend,
        "source":source["request"]["source"],"app_env":"test","config_dir":configuration["directory"]}))?;
    if physical != source["generation"]["physical_binding"] {
        return Err("当前请求配置改变了已登记C52完整物理来源".into());
    }
    if state::load(&run, true)? != saved
        || running::history::predecessor(backend, &run, &saved)? != source
    {
        return Err("当前请求构造期间来源历史或存储绑定变化".into());
    }
    configuration::verify(backend, &value)?;
    configuration::execution_source(backend, &value)?;
    Ok(value)
}

fn matches_build_context(
    recorded: &Value,
    current: &Value,
    request: &Value,
    source: &Value,
) -> bool {
    if recorded == current {
        return true;
    }

    // 原来源已有编译和实际运行证明，允许调用环境变化时继续使用同一组冻结输入。
    let Some(root) = source["seed_target"]["execution_backend"]["path"].as_str() else {
        return false;
    };
    let original_inputs = [
        ("backend_build", &source["request"]["backend_build"]),
        ("maintenance_build", &source["request"]["maintenance_build"]),
        (
            "source_environment",
            &source["registration"]["source_environment"],
        ),
    ];
    if request["execution_backend"].as_str() != Some(root)
        || !request["adapter_contract"].is_null()
        || !request["product_backend"].is_null()
        || !original_inputs
            .into_iter()
            .all(|(field, binding)| binding.is_object() && &request[field] == binding)
    {
        return false;
    }

    let Some((recorded, current)) = recorded.as_object().zip(current.as_object()) else {
        return false;
    };
    recorded.len() == current.len()
        && recorded.get("environment").is_some_and(Value::is_object)
        && current.get("environment").is_some_and(Value::is_object)
        && recorded
            .iter()
            .all(|(key, value)| key == "environment" || current.get(key) == Some(value))
}

fn publish(backend: &Path, output: &Path, args: &Value, request: &Value) -> Result<()> {
    validate_output(backend, output)?;
    let registration = common::bound_file(backend, &request["source_registration"], "来源原登记")?;
    if registration
        .parent()
        .and_then(Path::parent)
        .is_some_and(|run| output.starts_with(run))
    {
        return Err("generation-request外部收据不得写入来源复制账本目录".into());
    }
    if build(args)? != *request {
        return Err("来源请求发布前输入发生变化".into());
    }
    let pending = output.with_file_name(format!(
        ".{}.{}.pending",
        output
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("来源请求输出名称无效")?,
        uuid::Uuid::new_v4()
    ));
    crate::recovery::inputs::document::write_pending_and_link(output, request, &pending)?;
    let bytes = crate::recovery::inputs::document::canonical_bytes(request)?;
    crate::recovery::inputs::document::verify_published(output, request, &bytes)?;
    if document::read(output)? != *request || build(args)? != *request {
        return Err("来源请求发布后输入漂移；保留已创建收据和pending，禁止重放".into());
    }
    if std::fs::read(&pending)? != bytes {
        return Err("来源请求pending身份内容变化，保留现场禁止清理或重放".into());
    }
    std::fs::remove_file(pending)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frozen_inputs() -> (Value, Value, Value) {
        let root = "D:/frozen/backend";
        let binding = |file: &str, digit: &str| json!({"path":format!("{root}/.local-tests/{file}"),"bytes":100,"sha256":digit.repeat(64)});
        let request = json!({"execution_backend":root,"adapter_contract":null,"product_backend":null,
            "backend_build":binding("backend-build.json","a"),
            "maintenance_build":binding("maintenance.json","b"),
            "source_environment":binding("source-environment.json","c")});
        let source = json!({"request":{"backend_build":request["backend_build"],
            "maintenance_build":request["maintenance_build"]},
            "registration":{"source_environment":request["source_environment"]},
            "seed_target":{"execution_backend":{"path":root}}});
        let context = json!({"commands":{"api":["cargo","build"],"worker":["cargo","build"]},
            "profile":"dev","target":"x86_64-pc-windows-msvc","jobs":"1",
            "toolchain":{"cargo":"cargo 1.98.1","rustc":"rustc 1.98.1"},
            "environment":{"variables":["Path"],"sha256":"d".repeat(64)}});
        (request, source, context)
    }

    #[test]
    fn matches_original_frozen_generation_when_only_the_environment_changes() {
        let (request, source, recorded) = frozen_inputs();
        let mut current = recorded.clone();
        current["environment"] =
            json!({"variables":["LIBCLANG_PATH","Path"],"sha256":"e".repeat(64)});
        assert!(matches_build_context(
            &recorded, &current, &request, &source
        ));

        for field in ["backend_build", "maintenance_build", "source_environment"] {
            let mut invalid = request.clone();
            invalid[field]["sha256"] = json!("f".repeat(64));
            assert!(
                !matches_build_context(&recorded, &current, &invalid, &source),
                "{field}"
            );
            invalid.as_object_mut().unwrap().remove(field);
            assert!(
                !matches_build_context(&recorded, &current, &invalid, &source),
                "{field}"
            );
        }
        for field in ["execution_backend", "adapter_contract", "product_backend"] {
            let mut invalid = request.clone();
            invalid[field] = json!("different");
            assert!(
                !matches_build_context(&recorded, &current, &invalid, &source),
                "{field}"
            );
        }
    }

    #[test]
    fn matches_exact_context_and_rejects_other_context_changes() {
        let (request, source, recorded) = frozen_inputs();
        assert!(matches_build_context(
            &recorded,
            &recorded,
            &Value::Null,
            &Value::Null
        ));

        for field in ["commands", "profile", "target", "jobs"] {
            let mut current = recorded.clone();
            current["environment"]["sha256"] = json!("e".repeat(64));
            current[field] = json!("different");
            assert!(
                !matches_build_context(&recorded, &current, &request, &source),
                "{field}"
            );
        }
        for tool in ["cargo", "rustc"] {
            let mut current = recorded.clone();
            current["environment"]["sha256"] = json!("e".repeat(64));
            current["toolchain"][tool] = json!("1.100.0");
            assert!(
                !matches_build_context(&recorded, &current, &request, &source),
                "{tool}"
            );
        }
    }
}

//! 六文件来源登记核心；控制器授权、真实 quiet 观察和模块挂接由父流程负责。
use std::path::Path;

use serde_json::{Value, json};

use super::{anchors, publication, registry, request};
use crate::{
    Result,
    recovery::clone::native::{
        bridge,
        export::{
            input::{self as export_input, Context},
            proof,
        },
        json as document,
        lifecycle::{manifest, state},
        resources::common,
        target_verify::{guard::Generation, published},
    },
};

const PUBLIC_FIELDS: [&str; 12] = [
    "run_manifest",
    "source_to_seed",
    "post_copy",
    "post_verify",
    "seed_registration",
    "seed_registration_stage",
    "capacity",
    "departments",
    "identity",
    "seed_handoff",
    "seed_close",
    "source_environment",
];

struct Session<'a> {
    backend: &'a Path,
    run: &'a Path,
    current: u64,
    manifest: manifest::Manifest,
    state: Value,
    state_binding: Value,
    generation: &'a Generation,
}

struct Derived {
    value: Value,
    original: Value,
    initialized: Value,
    target: Value,
    review: Value,
    bindings: Vec<Value>,
}

struct Files {
    lineage: Value,
    storage: Value,
    registry: Value,
    request: Value,
    generation: Value,
}

struct FirstRound {
    files: Files,
    derived: Derived,
    registry: Value,
    generation: Value,
    runtime: Value,
}

pub(crate) fn materialize(
    backend: &Path,
    run: &Path,
    current: u64,
    coordinator_tool: &Value,
    scope: &bridge::scope::Scope,
    generation: &Generation,
    quiet: &mut dyn FnMut(&Value) -> Result<()>,
) -> Result<Value> {
    scope.assert_unchanged()?;
    let result = execute(
        backend,
        run,
        current,
        coordinator_tool,
        scope,
        generation,
        quiet,
    );
    let unchanged = scope.assert_unchanged();
    let result = result?;
    unchanged?;
    Ok(result)
}

fn execute(
    backend: &Path,
    run: &Path,
    current: u64,
    coordinator_tool: &Value,
    scope: &bridge::scope::Scope,
    generation: &Generation,
    quiet: &mut dyn FnMut(&Value) -> Result<()>,
) -> Result<Value> {
    let session = Session::load(backend, run, current, generation)?;
    let output = publication::output(backend, run, &session.state, current)?;
    let before = anchors::collect(
        backend,
        run,
        &session.manifest.value,
        current,
        coordinator_tool,
        generation,
        quiet,
    )?;
    let first = publish_round(&session, &output, &before, scope)?;
    let after = anchors::collect(
        backend,
        run,
        &session.manifest.value,
        current,
        coordinator_tool,
        generation,
        quiet,
    )?;
    confirm_round(&session, &before, &after, &first, scope)?;
    let registration = registration_document(run, &after.anchors, &first.files)?;
    let binding =
        publication::write_or_match(backend, &output, "source-registration.json", &registration)?;
    session.recheck()?;
    Ok(result_document(&first.files, binding))
}

fn publish_round(
    session: &Session<'_>,
    output: &Path,
    before: &anchors::Anchors,
    scope: &bridge::scope::Scope,
) -> Result<FirstRound> {
    let backend = session.backend;
    let run = session.run;
    let current = session.current;
    let lineage = publication::write_or_match(
        backend,
        output,
        "producer-lineage.json",
        &before.anchors["lineage"],
    )?;
    let storage = publication::write_or_match(
        backend,
        output,
        "source-storage.json",
        &storage_document(run, &before.anchors)?,
    )?;
    let runtime = file_binding(backend, &before.runtime.join("runtime.json"))?;
    let registry = build_registry(before, &runtime)?;
    let registry_binding =
        publication::write_or_match(backend, output, "producers.json", &registry)?;
    let derived = Derived::load(session, before, &runtime, &registry, &registry_binding)?;
    let request =
        publication::write_or_match(backend, output, "source-request.json", &derived.value)?;
    let context = Context::from_source_registration(
        backend,
        run,
        current,
        &request,
        &before.anchors["source_environment"],
    )?;
    let generation = proof::generation_in(&context, scope)?;
    if serde_json::to_value(&context.environment)? != before.environment {
        return Err("来源登记的执行环境与已核验运行环境不一致".into());
    }
    let generation_binding =
        publication::write_or_match(backend, output, "generation-verified.json", &generation)?;
    let files = Files {
        lineage,
        storage,
        registry: registry_binding,
        request,
        generation: generation_binding,
    };
    Ok(FirstRound {
        files,
        derived,
        registry,
        generation,
        runtime,
    })
}

fn confirm_round(
    session: &Session<'_>,
    before: &anchors::Anchors,
    after: &anchors::Anchors,
    first: &FirstRound,
    scope: &bridge::scope::Scope,
) -> Result<()> {
    let backend = session.backend;
    let run = session.run;
    let current = session.current;
    if before.environment != after.environment {
        return Err("来源登记期间执行环境变化".into());
    }
    compare_anchors(&before.anchors, &after.anchors)?;
    let expected_registry = build_registry(after, &first.runtime)?;
    let expected = Derived::load(
        session,
        after,
        &first.runtime,
        &expected_registry,
        &first.files.registry,
    )?;
    compare_documents(
        backend,
        &first.files,
        &first.registry,
        &expected_registry,
        &first.derived.value,
        &expected.value,
    )?;
    let repeated = Context::from_source_registration(
        backend,
        run,
        current,
        &first.files.request,
        &after.anchors["source_environment"],
    )?;
    if proof::generation_in(&repeated, scope)? != first.generation {
        return Err("seed 源 generation 验证在登记期间变化".into());
    }
    first.derived.recheck(session)?;
    expected.recheck(session)?;
    for binding in [
        &first.files.lineage,
        &first.files.storage,
        &first.files.registry,
        &first.files.request,
        &first.files.generation,
    ] {
        common::bound_file(backend, binding, "source-register 已写证据")?;
    }
    session.recheck()?;
    scope.assert_unchanged()?;
    Ok(())
}

impl<'a> Session<'a> {
    fn load(
        backend: &'a Path,
        run: &'a Path,
        current: u64,
        generation: &'a Generation,
    ) -> Result<Self> {
        common::local_directory(backend, run, "source-register 运行目录")?;
        let manifest = manifest::load(backend, &run.join("manifest.json"))?;
        let state_binding = file_binding(backend, &run.join("state.json"))?;
        let saved = state::load(run, true)?;
        reject_prior(&saved, current)?;
        common::bound_file(backend, &state_binding, "source-register 固定阶段状态")?;
        let session = Self {
            backend,
            run,
            current,
            manifest,
            state: saved,
            state_binding,
            generation,
        };
        session.recheck()?;
        Ok(session)
    }

    fn recheck(&self) -> Result<()> {
        let initialized = common::bound_file(
            self.backend,
            &self.manifest.value["initialized"],
            "来源登记初始化目标",
        )?;
        self.generation
            .protects(initialized.parent().ok_or("初始化目标缺少目录")?)?;
        let after = manifest::load(self.backend, &self.run.join("manifest.json"))?;
        if after.value != self.manifest.value
            || after.digest != self.manifest.digest
            || state::load(self.run, true)? != self.state
        {
            return Err("source-register 固定清单或阶段状态变化；保留前缀，禁止重放".into());
        }
        common::bound_file(
            self.backend,
            &self.state_binding,
            "source-register 固定阶段状态",
        )?;
        reject_prior(&self.state, self.current)
    }
}

fn reject_prior(saved: &Value, current: u64) -> Result<()> {
    if current == 0 {
        return Err("source-register 当前 attempt 必须为正整数".into());
    }
    if state::attempts(saved)?.iter().any(|item| {
        item["stage"] == "seed-runtime"
            && item["mode"] == "source-register"
            && item["number"].as_u64() != Some(current)
            && item["status"] == "passed"
    }) {
        return Err("同一 seed run 已发布源登记，不能生成第二份".into());
    }
    Ok(())
}

fn source_configuration(backend: &Path, runtime: &Value, target: &Value) -> Result<Value> {
    use super::super::super::s3::app_configuration;
    let runtime = common::read_bound(backend, runtime, "seed运行配置")?;
    let binding = app_configuration::configuration_value(
        backend,
        Path::new(document::text(&runtime["api_config_dir"], "seed配置目录")?),
        "test",
    )?;
    let objects = app_configuration::private_configuration(backend, &binding)?;
    if objects != target["target"]["s3"] {
        return Err("seed源运行对象配置与已初始化目标身份不同".into());
    }
    Ok(binding)
}

impl Derived {
    fn load(
        session: &Session<'_>,
        anchors: &anchors::Anchors,
        runtime: &Value,
        registry: &Value,
        registry_binding: &Value,
    ) -> Result<Self> {
        let backend = session.backend;
        let manifest = &session.manifest.value;
        let original = common::read_bound(backend, &manifest["source_request"], "原始私有源请求")?;
        export_input::validate_request(backend, &original)?;
        let path = common::bound_file(backend, &manifest["initialized"], "seed 原初始化")?;
        let (initialized, target) = published::owned_history(backend, &path, session.generation)?;
        let review = published::request_binding(backend, &target)?;
        let maintenance =
            common::read_bound(backend, &target["maintenance_build"], "seed 目标维护构建")?;
        let processes = json!({
            "api":file_binding(backend, &anchors.runtime.join("api.json"))?,
            "worker":file_binding(backend, &anchors.runtime.join("worker.json"))?
        });
        let configuration = source_configuration(backend, runtime, &target)?;
        let value = request::build(request::Inputs {
            manifest,
            original_request: &original,
            target_request: &target,
            post_request: &anchors.post,
            runtime: &anchors.runtime,
            runtime_binding: runtime,
            object_configuration: &configuration,
            handoff: &anchors.handoff,
            registry,
            registry_binding,
            maintenance: &maintenance,
            process_bindings: &processes,
        })?;
        export_input::validate_request(backend, &value)?;
        let bindings = vec![
            manifest["source_request"].clone(),
            manifest["initialized"].clone(),
            target["maintenance_build"].clone(),
            anchors.anchors["post_copy"].clone(),
            anchors.anchors["seed_handoff"].clone(),
            anchors.anchors["source_environment"].clone(),
            runtime.clone(),
            processes["api"].clone(),
            processes["worker"].clone(),
            registry_binding.clone(),
        ];
        let result = Self {
            value,
            original,
            initialized,
            target,
            review,
            bindings,
        };
        result.recheck(session)?;
        Ok(result)
    }

    fn recheck(&self, session: &Session<'_>) -> Result<()> {
        if common::read_bound(
            session.backend,
            &session.manifest.value["source_request"],
            "原始私有源请求",
        )? != self.original
        {
            return Err("来源登记原始源请求变化".into());
        }
        export_input::validate_request(session.backend, &self.original)?;
        for binding in &self.bindings {
            common::bound_file(session.backend, binding, "派生源原始输入")?;
        }
        let path = common::bound_file(
            session.backend,
            &session.manifest.value["initialized"],
            "seed 原初始化",
        )?;
        let (initialized, target) =
            published::owned_history(session.backend, &path, session.generation)?;
        if initialized != self.initialized
            || target != self.target
            || published::request_binding(session.backend, &target)? != self.review
        {
            return Err("派生源初始化历史或原请求在登记期间变化".into());
        }
        Ok(())
    }
}

fn file_binding(backend: &Path, path: &Path) -> Result<Value> {
    let path = common::common_file(backend, path, "source-register 输入文件")?;
    common::binding(&path)
}

fn build_registry(anchors: &anchors::Anchors, runtime: &Value) -> Result<Value> {
    registry::build(
        &anchors.anchors["lineage"],
        &anchors.runtime,
        runtime,
        document::text(&anchors.handoff["scope_id"], "seed handoff scope")?,
    )
}

fn compare_anchors(before: &Value, after: &Value) -> Result<()> {
    if before != after {
        return Err("seed 源登记期间原证据、lineage 或存储代次发生变化".into());
    }
    Ok(())
}

fn compare_documents(
    backend: &Path,
    files: &Files,
    registry: &Value,
    expected_registry: &Value,
    request: &Value,
    expected_request: &Value,
) -> Result<()> {
    if registry != expected_registry
        || request != expected_request
        || common::read_bound(
            backend,
            &files.registry,
            "source-register producer registry",
        )? != *expected_registry
        || common::read_bound(backend, &files.request, "source-register 私有源请求")?
            != *expected_request
    {
        return Err("seed 源请求或 producer registry 在生成期间变化".into());
    }
    Ok(())
}

fn storage_document(run: &Path, anchors: &Value) -> Result<Value> {
    let manifest = anchors
        .get("run_manifest")
        .ok_or("来源锚点缺少 run_manifest")?;
    let storage = anchors.get("storage").ok_or("来源锚点缺少 storage")?;
    Ok(
        json!({"format_version":1,"kind":"devex-clone-seed-source-storage",
        "run_directory":run,"run_manifest":manifest,"side":"target","storage":storage}),
    )
}

fn registration_document(run: &Path, anchors: &Value, files: &Files) -> Result<Value> {
    let mut result = json!({"format_version":1,"kind":"devex-clone-seed-source-registration",
        "run_directory":run,"producer_lineage":files.lineage,"source_storage":files.storage,
        "source_request":files.request,"generation_verified":files.generation,
        "remote_writes":0,"outbox_drained":true,"restore_qualified":false});
    for field in PUBLIC_FIELDS {
        result[field] = anchors
            .get(field)
            .ok_or("来源锚点缺少正式登记字段")?
            .clone();
    }
    Ok(result)
}

fn result_document(files: &Files, registration: Value) -> Value {
    json!({"status":"seed_source_registered","registration":registration,"source_request":files.request,
        "source_storage":files.storage,"generation_verified":files.generation,
        "remote_writes":0,"outbox_drained":true,"restore_qualified":false})
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/source_registration/materialize.rs"]
mod tests;

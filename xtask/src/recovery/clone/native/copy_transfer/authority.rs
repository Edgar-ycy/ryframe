//! 复制阶段只从正式运行清单、当前控制器和原导出取得输入。
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::super::{
    bridge::scope::Scope,
    cache,
    copy_ledger::session::Mode,
    digest, export, json as document,
    lifecycle::{identity, lock, manifest, stage::StageRequest, state},
    resources::{common, storage::registered},
    s3::app_configuration,
    target_verify::{guard::Inputs, input::request},
};
use crate::{
    Result,
    cli::CloneSide,
    local_test_path::{LocalTestPathKind, strip_local_test_prefix, validate_local_test_path},
    recovery::source::evidence::comparison,
};

pub(super) struct Stage<'a> {
    pub(super) backend: &'a Path,
    pub(super) run: &'a Path,
    pub(super) mode: Mode,
    pub(super) manifest: Value,
    pub(super) copy: PathBuf,
    pub(super) descriptor: Value,
    pub(super) scope: Scope,
    saved: Value,
    attempt: Option<u64>,
    controller: Option<(Value, Value)>,
    held: Inputs,
}

impl<'a> Stage<'a> {
    pub(super) fn load(request: &StageRequest<'a>) -> Result<Self> {
        if request.stage != "copy" || request.attempt == Some(0) {
            return Err("复制阶段必须绑定正式 copy 入口与阶段序号".into());
        }
        let mode = match request.mode {
            "run" => Mode::New,
            "reconcile" => Mode::Reconcile,
            "resume" => Mode::Resume,
            _ => return Err("复制阶段模式无效".into()),
        };
        let backend = request.backend;
        let run = request.run_dir;
        let manifest_path = run.join("manifest.json");
        let registered = manifest::load(backend, &manifest_path)?;
        let manifest = registered.value;
        let state_path = run.join("state.json");
        let state_binding = common::binding(&state_path)?;
        let saved = state::load(run, true)?;
        let controller = request
            .attempt
            .map(|number| current_controller(run, &saved, number, request.mode))
            .transpose()?;
        let mut paths = vec![manifest_path.clone(), state_path.clone()];
        if let Some(number) = request.attempt {
            paths.push(state::controller_path(run, number));
            paths.push(run.join("run.lock/owner.json"));
        }
        let held = Inputs::open(paths)?;
        if digest::file(&manifest_path)? != registered.digest
            || common::binding(&state_path)? != state_binding
        {
            return Err("复制清单或当前阶段状态在建立保护期间变化".into());
        }
        let copy = PathBuf::from(document::text(&manifest["copy_directory"], "固定复制目录")?);
        let expected = if mode == Mode::New {
            LocalTestPathKind::NewDirectory
        } else {
            LocalTestPathKind::ExistingDirectory
        };
        validate_local_test_path(&copy, backend, expected)?;
        let descriptor = export_descriptor(backend, run, &manifest, &saved)?;
        for (binding, label) in [
            (&descriptor, "源导出"),
            (&manifest["initialized"], "目标初始化"),
        ] {
            let path = common::bound_file(backend, binding, label)?;
            let evidence = path.parent().ok_or("复制证据缺少受管父目录")?;
            if strip_local_test_prefix(&copy, evidence).is_some()
                || strip_local_test_prefix(evidence, &copy).is_some()
            {
                return Err("复制输出不得与原导出或目标初始化证据目录重叠".into());
            }
        }
        let direct = document::array(&manifest["build_bridges"], "复制构建桥接")?;
        let inherited = cache::runner::inherited_bridges(backend, &manifest)?;
        let scope = Scope::load(backend, direct, &inherited)?;
        let stage = Self {
            backend,
            run,
            mode,
            manifest,
            copy,
            descriptor,
            scope,
            saved,
            attempt: request.attempt,
            controller,
            held,
        };
        stage.check()?;
        Ok(stage)
    }

    pub(super) fn check(&self) -> Result<()> {
        self.held.check()?;
        if let Some(number) = self.attempt {
            let row = state::running_attempt(&self.saved)?.ok_or("当前复制阶段缺失")?;
            let actual = current_controller(
                self.run,
                &self.saved,
                number,
                document::text(&row["mode"], "当前复制模式")?,
            )?;
            if Some(&actual) != self.controller.as_ref() {
                return Err("原复制控制器在持锁期间变化".into());
            }
        }
        Ok(())
    }

    pub(super) fn preflight(&self) -> Result<()> {
        if self.mode != Mode::New {
            let files = super::record::Files::bind(self.backend, &self.manifest, &self.descriptor)?;
            let session = files.read_record()?;
            super::super::copy_ledger::check_continuation(
                self.backend,
                &self.copy.join("ledger"),
                document::text(&session["plan_sha256"], "原计划摘要")?,
                document::text(&session["generation_sha256"], "原复制代次")?,
                self.mode,
            )?;
        }
        self.check()
    }
}

fn current_controller(
    run: &Path,
    saved: &Value,
    number: u64,
    mode: &str,
) -> Result<(Value, Value)> {
    let row = state::running_attempt(saved)?.ok_or("复制阶段尚未取得原生控制权")?;
    if row["number"].as_u64() != Some(number) || row["stage"] != "copy" || row["mode"] != mode {
        return Err("复制请求不属于同一运行中的阶段和模式".into());
    }
    let (binding, owner) = state::controller_record(run, number, row)?;
    let (_, active) = lock::owner_observation(run)?.ok_or("复制阶段缺少外层运行锁")?;
    if owner != active || owner["identity"] != identity::current()? {
        return Err("复制只能由当前登记的原生控制器执行".into());
    }
    Ok((binding, owner))
}

fn export_descriptor(backend: &Path, run: &Path, manifest: &Value, saved: &Value) -> Result<Value> {
    let descriptor = if manifest["source_export"].is_null() {
        let row = state::attempts(saved)?
            .iter()
            .rfind(|row| row["stage"] == "export" && row["status"] == "passed")
            .ok_or("复制前须完成已登记的原导出阶段")?;
        let number = row["number"].as_u64().ok_or("原导出阶段序号无效")?;
        let path = common::bound_file(backend, &row["result"], "原导出阶段结果")?;
        if path != state::result_path(run, number) {
            return Err("原导出结果不属于固定运行和阶段序号".into());
        }
        document::read(&path)?["export"].clone()
    } else {
        manifest["source_export"].clone()
    };
    let exported = common::read_bound(backend, &descriptor, "固定复制源导出")?;
    if exported["request"] != manifest["source_request"] {
        return Err("固定导出不属于本阶段登记的原源请求".into());
    }
    Ok(descriptor)
}

pub(super) struct Source {
    pub(super) export: Value,
    pub(super) root: PathBuf,
    pub(super) request: Value,
    pub(super) execution: PathBuf,
    pub(super) frozen: export::frozen::FrozenExport,
    pub(super) configuration: Value,
    pub(super) endpoint: String,
    pub(super) region: String,
    storage: Option<Value>,
    held: Inputs,
}

impl Source {
    pub(super) fn load(stage: &Stage<'_>) -> Result<Self> {
        let backend = stage.backend;
        let context = export::input::Context::for_copy(backend, stage.run, &stage.copy)?;
        let export = common::read_bound(backend, &stage.descriptor, "原复制导出")?;
        let frozen = export::frozen::verify(backend, &stage.descriptor)?;
        if frozen.request != context.request || context.manifest != stage.manifest {
            return Err("原冻结导出、源请求与当前复制清单不同".into());
        }
        let root = common::local_directory(
            backend,
            Path::new(document::text(&export["artifact_root"], "原导出证据根")?),
            "原导出证据根",
        )?;
        let mut request = context.request.clone();
        request["target"] = context.source().clone();
        request["execution_backend"] = json!({"path":context.execution_backend});
        let configuration = context.source()["s3"]["configuration"].clone();
        let public = app_configuration::public_configuration(backend, &configuration)?;
        request["config_dir"] = configuration["directory"].clone();
        let held = protect_source(&context)?;
        let source = Self {
            export,
            root,
            request,
            execution: context.execution_backend,
            frozen,
            endpoint: document::text(&public["endpoint"], "源对象端点")?.to_owned(),
            region: document::text(&public["region"], "源对象区域")?.to_owned(),
            configuration,
            storage: source_storage(stage)?,
            held,
        };
        source.check_generation(stage)?;
        Ok(source)
    }

    pub(super) fn check_inputs(&self) -> Result<()> {
        self.held.checkpoint()
    }

    pub(super) fn check_generation(&self, stage: &Stage<'_>) -> Result<()> {
        stage.check()?;
        self.check_inputs()?;
        let generation =
            export::proof::copy::request(&json!({"backend":stage.backend,"run_dir":stage.run}))?;
        if generation != self.frozen.generation || source_storage(stage)? != self.storage {
            return Err("当前源停止证据或已登记的源存储代次不同于原复制来源".into());
        }
        stage.check()?;
        self.check_inputs()
    }
}

fn source_storage(stage: &Stage<'_>) -> Result<Option<Value>> {
    let local = registered::current(stage.backend, stage.run, CloneSide::Source)?;
    if stage.manifest["copy_stage"] != "seed_to_arm" {
        return Ok(local);
    }
    if local.is_some() {
        return Err("测量臂只能继承已登记 seed 来源存储".into());
    }
    let published = comparison::copy_source(stage.backend, &stage.manifest)?;
    let directory = Path::new(document::text(
        &published["directory"],
        "继承 seed 运行目录",
    )?);
    let inherited = registered::current(stage.backend, directory, CloneSide::Target)?
        .ok_or("继承 seed RustFS 没有同代运行证据")?;
    if published["source_request"] != stage.manifest["source_request"]
        || published["storage"]["storage"] != inherited
    {
        return Err("原 seed 来源请求或实际存储与继承的登记不同".into());
    }
    Ok(Some(inherited))
}

fn protect_source(context: &export::input::Context) -> Result<Inputs> {
    let mut executables = BTreeMap::new();
    for (name, binding) in document::object(&context.request["tools"], "源外部工具")? {
        executables.insert(
            context.tool(name)?,
            document::text(&binding["sha256"], "源工具摘要")?.to_owned(),
        );
    }
    for (root, field, roles) in [
        (
            context.backend.as_path(),
            "backend_build",
            &["api", "worker"][..],
        ),
        (
            context.execution_backend.as_path(),
            "maintenance_build",
            &["reset", "migrate", "tenant-data"][..],
        ),
    ] {
        let build = common::read_bound(root, &context.request[field], "源受管构建")?;
        let artifacts = document::exact(&build["artifacts"], roles, "源构建角色")?;
        for artifact in artifacts.values() {
            let binding = json!({"path":artifact["executable"],"sha256":artifact["sha256"]});
            let executable = request::external_file(&binding)?;
            let sha = document::text(&binding["sha256"], "源构建产物摘要")?.to_owned();
            if let Some(old) = executables.insert(executable, sha.clone())
                && old != sha
            {
                return Err("源构建与外部工具重复绑定同一路径但摘要不同".into());
            }
        }
    }
    let binary = crate::recovery::fixture_control::portable_path(&std::env::current_exe()?);
    executables.insert(binary.clone(), digest::file(&binary)?.sha256);
    let mut paths = source_files(context)?;
    paths.extend(executables.keys().cloned());
    let held = Inputs::open(paths)?;
    for (path, expected) in executables {
        if digest::file(&path)?.sha256 != expected {
            return Err("源受管二进制在建立保护期间变化".into());
        }
    }
    held.check()?;
    Ok(held)
}

fn source_files(context: &export::input::Context) -> Result<BTreeSet<PathBuf>> {
    let mut paths = BTreeSet::from([context.request_path.clone()]);
    for descriptor in [
        &context.manifest["source_environment"],
        &context.request["backend_build"],
        &context.request["runtime"],
        &context.request["processes"]["api"],
        &context.request["processes"]["worker"],
        &context.request["producers_registry"],
    ] {
        paths.insert(common::bound_file(
            &context.backend,
            descriptor,
            "原复制来源文件",
        )?);
    }
    paths.insert(common::bound_file(
        &context.execution_backend,
        &context.request["maintenance_build"],
        "原复制来源维护构建",
    )?);
    let runtime = common::read_bound(
        &context.backend,
        &context.request["runtime"],
        "原复制来源运行",
    )?;
    for role in ["api", "worker"] {
        let (_, files) = app_configuration::runtime_configuration(
            &context.backend,
            &context.execution_backend,
            &context.environment,
            &runtime,
            role,
        )?;
        let directory = Path::new(document::text(
            &runtime[format!("{role}_config_dir")],
            "来源角色配置目录",
        )?);
        paths.extend(files.keys().map(|name| directory.join(name)));
    }
    let configuration = &context.source()["s3"]["configuration"];
    let directory = Path::new(document::text(
        &configuration["directory"],
        "原复制来源对象配置目录",
    )?);
    paths.extend(
        document::object(&configuration["files"], "原复制来源对象配置文件")?
            .keys()
            .map(|name| directory.join(name)),
    );
    for database in context.databases()? {
        paths.insert(common::common_file(
            &context.backend,
            Path::new(document::text(
                &database["defaults_file"],
                "原复制来源 MySQL 配置",
            )?),
            "原复制来源 MySQL 配置",
        )?);
    }
    Ok(paths)
}

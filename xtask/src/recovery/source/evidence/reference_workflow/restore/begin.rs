//! 正式控制器在原停止与存储守卫内登记恢复，并保留精确的前像交接收据。
use std::{
    cell::Cell,
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::{Value, json};

use super::super::{local::Lock, path, proof, text};
use super::{
    Phase,
    begin_copy::{self, CopyWindow, VerifiedCopy},
    begin_proof, combine, expected_images, files,
    inputs::TargetSnapshot,
    native, object_images,
};
use crate::{
    Result,
    cli::RestoreBeginOptions,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        inputs::{
            RestoreChecks,
            document::{self, Document},
            reference,
        },
        runtime::restore_guard::registered_stopped_runtime,
    },
};
use native::{
    bridge::scope::Scope,
    cache::handoff::locks::Storage,
    copy_transfer::ReferenceTarget,
    export::transport::{CommandSpec, StdoutTarget, SystemTransport, Transport},
    resources::common,
    target_verify::guard::Inputs,
};

#[path = "begin/apply_inputs.rs"]
mod apply_inputs;
#[path = "begin/independent.rs"]
mod independent;
#[path = "begin/receipt.rs"]
mod receipt;
pub(super) use apply_inputs::{ApplySnapshot, PendingApply};
pub(super) use receipt::verify_record;

struct BeginInputs<'a> {
    root: &'a Path,
    reference: &'a Document,
    plan: &'a Value,
    backup_receipt: &'a Document,
    target: &'a Document,
    manifest: &'a Document,
    registration: &'a Document,
    product_plan: &'a Document,
}

impl<'a> From<&'a TargetSnapshot> for BeginInputs<'a> {
    fn from(snapshot: &'a TargetSnapshot) -> Self {
        Self {
            root: &snapshot.root,
            reference: &snapshot.chain.reference,
            plan: &snapshot.chain.reference.value,
            backup_receipt: &snapshot.chain.backup_document,
            target: &snapshot.target,
            manifest: &snapshot.manifest,
            registration: &snapshot.registration,
            product_plan: &snapshot.product_plan,
        }
    }
}

pub(in super::super) fn run(backend: &Path, options: &RestoreBeginOptions) -> Result<Value> {
    let request = json!({
        "plan": options.plan,
        "target_plan": options.target_plan,
        "runtime_registration": options.runtime_registration,
        "backup_root": options.backup_root,
    });
    let selection = super::inputs::RegisteredSelection::read(backend, &request)?;
    if reference::is_independent_b1(&selection.selected.reference.value) {
        return independent::run(backend, options, &request, selection);
    }
    let snapshot = TargetSnapshot::read(backend, &request, selection, true)?;
    let inputs = BeginInputs::from(&snapshot);
    let work = path(&inputs.plan["work_dir"])?;
    let maintenance = Maintenance::open(&options.maintenance_root, &options.maintenance_build)?;
    let normal_directory = receipt::directory(&inputs)?;
    let directory = if let Some(copy) = &options.copy {
        begin_copy::directory(normal_directory, copy, &snapshot.product_plan.value)?
    } else {
        normal_directory
    };
    let output = output_file(backend, work, &inputs, &directory, &options.output)?;
    files::work(&json!({"plan":inputs.plan}), true)?;
    if !output.parent().is_some_and(Path::is_dir) {
        return Err("恢复运行记录必须写到受管参考演练目录中".into());
    }
    validate_local_test_path(&directory, backend, LocalTestPathKind::NewDirectory)?;
    let lock = Lock::claim(work)?;
    let copy_window = options
        .copy
        .as_ref()
        .map(|copy| CopyWindow::open(backend, &snapshot, work, copy))
        .transpose()?;
    let copy = copy_window.as_ref().map(|window| &window.copy);
    if copy.is_some_and(|copy| output.starts_with(&copy.root)) {
        return Err("失败演练运行记录不能位于被检测的副本目录".into());
    }
    snapshot.assert_unchanged()?;
    let directory_created = Cell::new(false);
    let registered = registered_stopped_runtime(
        backend,
        &snapshot.registration.path,
        &document::binding(&snapshot.target),
        |stopped| {
            let active = Begin {
                backend,
                work,
                output: &output,
                directory: &directory,
                inputs: &inputs,
                maintenance: &maintenance,
                directory_created: &directory_created,
                copy,
                work_binding: None,
            };
            active.run_copy_target(&snapshot, &|| {
                lock.check()?;
                if let Some(window) = &copy_window {
                    window.check()?;
                }
                snapshot.check_runtime(&stopped(RestoreChecks::default())?)
            })
        },
    );
    let checked = combine(
        lock.check(),
        copy_window.as_ref().map_or(Ok(()), CopyWindow::check),
    );
    finish(
        backend,
        &directory,
        &directory_created,
        combine(registered, checked),
    )
}

fn finish(
    backend: &Path,
    directory: &Path,
    directory_created: &Cell<bool>,
    result: Result<(receipt::Intent, Document)>,
) -> Result<Value> {
    match result {
        Ok((intent, record)) => intent.finish(backend, &record),
        Err(error) if directory_created.get() => {
            combine(Err(error), receipt::record_unconfirmed(backend, directory))
        }
        Err(error) => Err(error),
    }
}

fn output_file(
    backend: &Path,
    work: &Path,
    inputs: &BeginInputs<'_>,
    evidence: &Path,
    output: &Path,
) -> Result<PathBuf> {
    let output = validate_local_test_path(output, backend, LocalTestPathKind::OutputFile)?;
    let initialized = path(&inputs.target.value["fresh_target"]["initialized"]["path"])?;
    let initialized = initialized.parent().ok_or("原目标初始化证据缺少目录")?;
    if !output.starts_with(&work)
        || output.starts_with(inputs.root)
        || output.starts_with(initialized)
        || output.starts_with(evidence)
    {
        return Err("恢复运行记录必须属于本参考演练目录的独立输出位置".into());
    }
    Ok(output)
}

struct Begin<'a> {
    backend: &'a Path,
    work: &'a Path,
    output: &'a Path,
    directory: &'a Path,
    inputs: &'a BeginInputs<'a>,
    maintenance: &'a Maintenance,
    directory_created: &'a Cell<bool>,
    copy: Option<&'a VerifiedCopy>,
    work_binding: Option<&'a Value>,
}

impl Begin<'_> {
    fn run_copy_target(
        &self,
        snapshot: &TargetSnapshot,
        stopped: &dyn Fn() -> Result<()>,
    ) -> Result<(receipt::Intent, Document)> {
        let arm = &snapshot.chain.arm;
        let bridges = native::json::array(&arm.request.value["build_bridges"], "恢复原构建桥接")?;
        let scope = Scope::load(self.backend, bridges, &[])?;
        let storage = Storage::acquire(self.backend, self.work, &arm.manifest.value)?;
        let result = (|| {
            let target = ReferenceTarget::open(
                self.backend,
                &arm.manifest.value,
                &storage,
                self.work,
                &scope,
            )?;
            let check = || {
                stopped()?;
                target.check()?;
                self.maintenance.check()
            };
            self.prepare(&target, &check)
        })();
        storage.finish(combine(result, scope.assert_unchanged()))
    }

    fn prepare(
        &self,
        target: &ReferenceTarget<'_>,
        check: &dyn Fn() -> Result<()>,
    ) -> Result<(receipt::Intent, Document)> {
        check()?;
        let (before, _) = expected_images::expected_images(
            self.inputs.plan,
            &self.inputs.manifest.value,
            target.initialized(),
        )?;
        common::ensure_new_directory(self.backend, self.directory, "正式登记证据目录")?;
        self.directory_created.set(true);
        let image = self.observe_before(target, before, check)?;
        self.register(target, &image, check)
    }

    fn observe_before(
        &self,
        target: &ReferenceTarget<'_>,
        before: Value,
        check: &dyn Fn() -> Result<()>,
    ) -> Result<Value> {
        let reference = self.inputs.plan;
        let objects = &reference["target"]["s3"];
        let storage = target.inputs();
        let object_directory = self.directory.join("objects");
        common::ensure_new_directory(self.backend, &object_directory, "开始登记对象前像证据")?;
        check()?;
        let (observed, _) = object_images::capture(
            &object_images::Observe {
                storage: &storage,
                configuration_root: &storage.execution_backend()?,
                configuration: target.configuration(),
                directory: &object_directory,
                scope: text(&reference["target"]["scope_id"])?,
                endpoint: text(&objects["endpoint"])?,
                region: text(&objects["region"])?,
                manifest: &self.inputs.manifest.value,
                expected: &before["objects"],
                plan_sha256: &native::digest::plan_hash(reference)?,
                generation_sha256: &native::digest::plan_hash(&target.initialized()["generation"])?,
            },
            Phase::Before,
            &SystemTransport,
            &|| target.checkpoint(),
        )?;
        let redis = target.redis_state()?;
        let actual = json!({"databases":before["databases"],"objects":observed,"redis":redis});
        if actual != before {
            return Err("开始登记的对象和 Redis 原初始化前像不一致".into());
        }
        check()?;
        Ok(actual)
    }

    fn register(
        &self,
        target: &ReferenceTarget<'_>,
        before: &Value,
        check: &dyn Fn() -> Result<()>,
    ) -> Result<(receipt::Intent, Document)> {
        let inputs = self.inputs;
        let selected = &inputs.plan["source"]["s3"]["configuration"];
        let registry = Configuration::open(self.backend, selected)?;
        let target_backend = target.inputs().execution_backend()?;
        let target_config = Configuration::open(&target_backend, target.configuration())?;
        let product_plan = self.execution_plan()?;
        let proof = begin_proof::build(
            &product_plan.value,
            &inputs.target.sha256(),
            text(&inputs.target.value["fresh_target"]["initialized"]["sha256"])?,
            before,
        )?;
        if registry.environment != target_config.environment {
            return Err("开始登记的来源和目标配置环境不一致".into());
        }
        let checkpoint = || -> Result<()> {
            check()?;
            registry.check()?;
            target_config.check()?;
            product_plan.assert_unchanged()
        };
        checkpoint()?;
        let intent = receipt::Intent::publish(
            self.backend,
            self.directory,
            inputs,
            self.output,
            &self.maintenance.binding,
            receipt::Evidence {
                image: before,
                proof: &proof,
                copy: self.copy,
                product_plan: &product_plan,
                work_binding: self.work_binding,
            },
        )?;
        let record = (|| -> Result<Document> {
            checkpoint()?;
            intent.check()?;
            output_file(self.backend, self.work, inputs, self.directory, self.output)?;
            if let Some(copy) = self.copy {
                self.register_copy(&registry, copy)?;
                checkpoint()?;
                intent.check()?;
            }
            self.invoke(&registry, &target_config, &product_plan, &intent.preimage)?;
            checkpoint()?;
            intent.check()?;
            let record = self.verify_running(&product_plan)?;
            self.maintenance.verify_source()?;
            record.assert_unchanged()?;
            Ok(record)
        })()?;
        Ok((intent, record))
    }

    fn verify_running(&self, product_plan: &Document) -> Result<Document> {
        let inputs = self.inputs;
        let mut reference = inputs.plan.clone();
        let manifest = if let Some(copy) = self.copy {
            reference["id"] = copy.manifest.value["id"].clone();
            &copy.manifest
        } else {
            inputs.manifest
        };
        let record = document::read(self.output, self.backend, "恢复运行记录")?;
        proof::record(&json!({
            "plan":reference,
            "manifest":manifest.value,
            "record":record.value,
            "product_plan":product_plan.value,
        }))?;
        Ok(record)
    }

    fn execution_plan(&self) -> Result<Document> {
        let original = self.inputs.product_plan;
        let Some(copy) = self.copy else {
            return Ok(original.clone());
        };
        let mut selected = original.value.clone();
        let id = &copy.manifest.value["id"];
        selected["id"] = id.clone();
        selected["backup_id"] = id.clone();
        let path = self.directory.join("copy-product-plan.json");
        native::json::publish_new(&path, &selected, "独立失败副本产品计划")?;
        let plan = document::read(&path, self.backend, "独立失败副本产品计划")?;
        if plan.value != selected {
            return Err("独立失败副本产品计划发布后内容不同".into());
        }
        Ok(plan)
    }

    fn register_copy(&self, registry: &Configuration<'_>, copy: &VerifiedCopy) -> Result<()> {
        let command = CommandSpec {
            program: self.maintenance.executable.clone(),
            args: vec![
                "--config-dir".into(),
                path_argument(&registry.directory)?,
                "backup-register".into(),
                "--manifest".into(),
                path_argument(&copy.manifest.path)?,
                "--backup-root".into(),
                path_argument(&copy.root)?,
            ],
            cwd: self.maintenance.root.clone(),
            environment: BTreeMap::from([("APP_ENV".into(), registry.environment.clone())]),
            stdin: None,
            stdout: StdoutTarget::Capture,
            work_dir: self.directory.to_path_buf(),
            label: "backup-register".into(),
            timeout: Duration::from_secs(1800),
        };
        let registered = SystemTransport.run(command);
        receipt::record_command(&self.directory.join("copy-registration.json"), &registered)?;
        match registered {
            Ok(output) if output.status.success() => Ok(()),
            _ => Err("独立失败副本登记未确认；请先只读核对原登记库".into()),
        }
    }

    fn invoke(
        &self,
        registry: &Configuration<'_>,
        target: &Configuration<'_>,
        product_plan: &Document,
        preimage: &Document,
    ) -> Result<()> {
        let command = CommandSpec {
            program: self.maintenance.executable.clone(),
            args: vec![
                "--config-dir".into(),
                path_argument(&registry.directory)?,
                "restore-begin".into(),
                "--plan".into(),
                path_argument(&product_plan.path)?,
                "--restore-config-dir".into(),
                path_argument(&target.directory)?,
                "--output".into(),
                path_argument(self.output)?,
                "--target-preimage-stdin".into(),
            ],
            cwd: self.maintenance.root.clone(),
            environment: BTreeMap::from([("APP_ENV".into(), registry.environment.clone())]),
            stdin: Some(preimage.raw.clone()),
            stdout: StdoutTarget::Capture,
            work_dir: self.directory.to_path_buf(),
            label: "restore-begin".into(),
            timeout: Duration::from_secs(1800),
        };
        let result = SystemTransport.run(command);
        receipt::record_command(&self.directory.join("command.json"), &result)?;
        match result {
            Ok(output) if output.status.success() => Ok(()),
            _ => Err("恢复开始登记未确认；请核对受管收据和原登记库，禁止直接重放".into()),
        }
    }
}

fn path_argument(path: &Path) -> Result<String> {
    Ok(path
        .to_str()
        .ok_or("恢复维护命令路径不是 UTF-8")?
        .to_owned())
}

struct Configuration<'a> {
    backend: &'a Path,
    binding: &'a Value,
    directory: PathBuf,
    environment: String,
    protection: Inputs,
}

impl<'a> Configuration<'a> {
    fn open(backend: &'a Path, binding: &'a Value) -> Result<Self> {
        let directory = path(&binding["directory"])?.to_path_buf();
        let environment = text(&binding["app_env"])?.to_owned();
        let verified =
            native::s3::app_configuration::configuration_value(backend, &directory, &environment)?;
        if verified != *binding {
            return Err("开始登记配置与原文件快照不同".into());
        }
        let files = document::object(&verified["files"], "开始登记配置文件清单")?;
        let protection = Inputs::open(files.keys().map(|name| directory.join(name)))?;
        let result = Self {
            backend,
            binding,
            directory,
            environment,
            protection,
        };
        result.check()?;
        Ok(result)
    }

    fn check(&self) -> Result<()> {
        self.protection.check()?;
        let current = native::s3::app_configuration::configuration_value(
            self.backend,
            &self.directory,
            &self.environment,
        )?;
        if current != *self.binding {
            return Err("开始登记配置与原文件快照不同".into());
        }
        Ok(())
    }
}

struct Maintenance {
    root: PathBuf,
    build: PathBuf,
    executable: PathBuf,
    proof: Value,
    binding: Value,
    protection: Inputs,
}

impl Maintenance {
    fn open(root: &Path, build: &Path) -> Result<Self> {
        let root = crate::recovery::fixture_control::portable_path(&root.canonicalize()?);
        let build = validate_local_test_path(build, &root, LocalTestPathKind::ExistingFile)?;
        let proof = native::maintenance::verify(&root, &build)?;
        let artifact = &proof["artifacts"]["tenant-data"];
        let executable = path(&artifact["executable"])?.to_path_buf();
        let protection = Inputs::open([build.clone(), executable.clone()])?;
        let descriptor = common::binding(&executable)?;
        if descriptor["bytes"] != artifact["bytes"]
            || descriptor["sha256"] != artifact["sha256"]
            || proof["source"]["snapshot"]["clean"] != true
        {
            return Err("开始登记维护程序必须绑定已验证的干净源码和实际二进制".into());
        }
        let binding = json!({
            "root":root, "build":common::binding(&build)?,
            "executable":descriptor, "source":proof["source"],
        });
        let result = Self {
            root,
            build,
            executable,
            proof,
            binding,
            protection,
        };
        result.check()?;
        Ok(result)
    }

    fn check(&self) -> Result<()> {
        self.protection.check()
    }

    fn verify_source(&self) -> Result<()> {
        self.check()?;
        if native::maintenance::verify(&self.root, &self.build)? != self.proof {
            return Err("开始登记前后的维护程序构建来源不同".into());
        }
        self.check()
    }
}

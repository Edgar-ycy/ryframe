//! 同一原生控制窗口核对角色配置、Worker 停止、存储及业务阶段的活动 API。

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{
    api_successor, context::Context, diagnostic::SeedVerifyCheckpoint, files, images, inspection,
    runtime, schedule_images,
};
use crate::{
    Result,
    recovery::{
        clone::native::{
            copy_transfer::observe_source::ReadWindow,
            export::transport::SystemTransport,
            input::BUCKETS,
            json as document,
            lifecycle::state,
            resources::common,
            s3::app_configuration,
            target_verify::{
                self, guard::Inputs, input::request_configuration, storage::StorageInputs,
            },
        },
        full_stack::worker::controller,
    },
};

pub(super) struct Live<'a, 'c> {
    pub(super) context: &'a Context<'c>,
    pub(super) execution: PathBuf,
    pub(super) configuration: Value,
    product: PathBuf,
    source_request: Value,
    environment: BTreeMap<String, String>,
    runtime: Value,
    api: Value,
    successor: Option<ActiveSuccessor>,
    roles: RoleWindow,
    files: Inputs,
}

impl<'a, 'c> Live<'a, 'c> {
    pub(super) fn load(context: &'a Context<'c>) -> Result<Self> {
        let diagnose = SeedVerifyCheckpoint::applies(context.stage, context.mode);
        SeedVerifyCheckpoint::SharedContext.check(diagnose, context.check())?;
        let environment =
            SeedVerifyCheckpoint::Configuration.check(diagnose, context.environment())?;
        let target = StorageInputs {
            backend: context.backend,
            request: &context.target,
            environment: &environment,
            observation_dir: &context.output,
        };
        let execution =
            SeedVerifyCheckpoint::Configuration.check(diagnose, target.execution_backend())?;
        let product = SeedVerifyCheckpoint::ApiBinding.check(diagnose, context.product_root())?;
        let (directory, _) = SeedVerifyCheckpoint::Configuration.check(
            diagnose,
            request_configuration::fresh_files(&execution, &environment, &context.target),
        )?;
        let configuration = SeedVerifyCheckpoint::Configuration.check(
            diagnose,
            app_configuration::configuration_value(&execution, &directory, "test"),
        )?;
        let mut source_request = context.source.request.clone();
        source_request["target"] = context.source.request["source"].clone();
        source_request["execution_backend"] = json!({"path":context.source.execution_backend});
        let (runtime, original_api, files) =
            SeedVerifyCheckpoint::ApiBinding.check(diagnose, api_bindings(context, &product))?;
        let successor =
            SeedVerifyCheckpoint::ApiBinding.check(diagnose, ActiveSuccessor::load(context))?;
        let api = successor
            .as_ref()
            .map_or(original_api, |active| active.tree["process"].clone());
        let guard = Self {
            context,
            execution,
            configuration,
            product,
            source_request,
            environment,
            runtime,
            api,
            successor,
            roles: RoleWindow::select(context.stage, context.mode),
            files,
        };
        guard.check_with_pre_layout_diagnostic(diagnose)?;
        Ok(guard)
    }

    pub(super) fn target(&self) -> StorageInputs<'_> {
        StorageInputs {
            backend: self.context.backend,
            request: &self.context.target,
            environment: &self.environment,
            observation_dir: &self.context.output,
        }
    }

    pub(super) fn source(&self) -> StorageInputs<'_> {
        StorageInputs {
            backend: self.context.backend,
            request: &self.source_request,
            environment: &self.context.source.environment,
            observation_dir: &self.context.output,
        }
    }

    pub(super) fn checkpoint(&self) -> Result<()> {
        self.context.owner.checkpoint()?;
        self.files.checkpoint()?;
        if let Some(successor) = &self.successor {
            successor.protected.checkpoint()?;
        }
        if self.roles == RoleWindow::ActiveApi {
            let pid = process_id(&self.api)?;
            if common::process_identity(pid)?.as_ref() != Some(&self.api) {
                return Err("目标活动 API 进程已退出或身份变化".into());
            }
        }
        Ok(())
    }

    pub(super) fn check(&self) -> Result<()> {
        self.check_with_pre_layout_diagnostic(false)
    }

    fn check_with_pre_layout_diagnostic(&self, diagnose: bool) -> Result<()> {
        let context = self.context;
        SeedVerifyCheckpoint::ApiBinding.check(diagnose, self.files.check())?;
        SeedVerifyCheckpoint::SharedContext.check(diagnose, context.check())?;
        let post_copy = context.stage == "post-copy";
        if post_copy {
            context.source_generation()?;
        }
        SeedVerifyCheckpoint::TargetGeneration.check(diagnose, self.check_target())?;
        SeedVerifyCheckpoint::StorageLayout.check(diagnose, images::target_storage(context))?;
        self.check_api()?;
        let check = || self.checkpoint();
        if post_copy {
            schedule_images::verify_source_and_target(
                &self.source(),
                &self.target(),
                &SystemTransport,
                &check,
            )?;
        } else {
            schedule_images::verify_target_databases(&self.target(), &SystemTransport, &check)?;
        }
        let scope = self.target().target_scope()?;
        let phase = format!("live-owner-{}", uuid::Uuid::new_v4().simple());
        let checkpoint = || self.checkpoint();
        ReadWindow::new(
            context.backend,
            &self.execution,
            &context.output,
            scope,
            &self.configuration,
            &SystemTransport,
            &checkpoint,
        )?
        .owners(BUCKETS, &phase)?;
        if context.mode.starts_with("departments-") {
            target_verify::publication::publish(
                &context.output.join(format!("{phase}-binding.json")),
                &self.department_target()?,
            )?;
        }
        self.check_api()?;
        self.files.check()?;
        context.check()
    }

    fn check_api(&self) -> Result<()> {
        let context = self.context;
        files::absent(&context.post.runtime.join("worker.json"))?;
        if runtime::contract(context.backend, &self.product, &context.post.runtime)? != self.runtime
        {
            return Err("API 运行合同或角色配置发生变化".into());
        }
        self.checkpoint()?;
        if self.roles != RoleWindow::ActiveApi {
            return Ok(());
        }
        if let Some(successor) = &self.successor {
            successor.check(&self.runtime)?;
        }
        inspection::require_actual_argv(
            &self.api,
            &json!([
                self.api["executable"],
                "--config-dir",
                self.runtime["api_config_dir"]
            ]),
        )?;
        runtime::listener(
            document::text(&context.selected["api_url"], "目标 API 地址")?,
            &self.api,
        )
    }

    pub(super) fn check_mutation(&self) -> Result<()> {
        self.files.check()?;
        self.check_target()?;
        self.check_api()?;
        self.files.check()
    }

    fn check_target(&self) -> Result<()> {
        images::target_identity(self.context, &self.target(), &self.configuration)?;
        if self.roles != RoleWindow::SeedRuntime {
            images::require_worker_closed(self.context)?;
        }
        Ok(())
    }

    pub(super) fn department_target(&self) -> Result<Value> {
        let target = self.target();
        let raw = &target.target()["s3"];
        let actual = app_configuration::public_configuration(&self.execution, &self.configuration)?;
        if actual["endpoint"] != raw["endpoint"] || actual["region"] != raw["region"] {
            return Err("部门对象证据的实际端点或区域不同于目标登记".into());
        }
        Ok(
            json!({"scope_id":target.target_scope()?,"endpoint":actual["endpoint"],
            "configuration_sha256":crate::recovery::clone::native::digest::plan_hash(raw)?}),
        )
    }
}

struct ActiveSuccessor {
    directory: PathBuf,
    tree: Value,
    protected: Inputs,
}

impl ActiveSuccessor {
    fn load(context: &Context<'_>) -> Result<Option<Self>> {
        let backend = context.backend;
        let Some(publication) =
            api_successor::load(backend, context.run, &context.post.descriptor)?
        else {
            return Ok(None);
        };
        let published = context
            .run
            .join("post-copy")
            .join("api-successors")
            .join("published.json");
        let mut paths = BTreeSet::from([published.clone()]);
        files::bindings(backend, &publication, &mut paths)?;
        let protected = Inputs::open(paths)?;
        files::bindings(backend, &publication, &mut BTreeSet::new())?;
        if document::read(&published)? != publication {
            return Err("活动 API 后继发布在取得保护前变化".into());
        }
        let directory = PathBuf::from(document::text(
            &publication["runtime_directory"],
            "活动 API 后继运行目录",
        )?);
        let tree = common::read_bound(backend, &publication["api_tree"], "活动 API 后继进程树")?;
        protected.check()?;
        Ok(Some(Self {
            directory,
            tree,
            protected,
        }))
    }

    fn check(&self, original_contract: &Value) -> Result<()> {
        self.protected.check()?;
        self.require_no_pending()?;
        // 两份运行合同和产物清单的原始字节相同，仍由外层核对原配置及产物。
        controller::require_api_tree(&self.directory, original_contract)?;
        for member in ["process", "supervisor", "monitor"] {
            let identity = &self.tree[member];
            if common::process_identity(process_id(identity)?)?.as_ref() != Some(identity) {
                return Err("已发布 API 后继的实际进程或监督树已变化".into());
            }
        }
        self.require_no_pending()?;
        self.protected.check()
    }

    fn require_no_pending(&self) -> Result<()> {
        for name in [
            "api-control.lock",
            "worker-control.lock",
            "worker.json",
            "worker-tree.json",
            "source-pair.json",
            "runtime-evidence.json",
        ] {
            files::absent(&self.directory.join(name))?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RoleWindow {
    ActiveApi,
    SeedPrepare,
    SeedRuntime,
}

impl RoleWindow {
    fn select(stage: &str, mode: &str) -> Self {
        match (stage, mode) {
            ("seed-runtime", "prepare") => Self::SeedPrepare,
            ("seed-runtime", "start" | "close") => Self::SeedRuntime,
            _ => Self::ActiveApi,
        }
    }
}

fn api_bindings(context: &Context<'_>, product: &Path) -> Result<(Value, Value, Inputs)> {
    let directory = &context.post.runtime;
    common::local_directory(context.backend, directory, "当前复制后 API 运行目录")?;
    let handoff = common::binding(&directory.join("handoff.json"))?;
    let saved = state::load(context.run, true)?;
    let mut prepared = None;
    for row in state::attempts(&saved)?.iter().rev().filter(|row| {
        row["stage"] == "post-copy" && row["mode"] == "prepare" && row["status"] == "passed"
    }) {
        let result = common::read_bound(context.backend, &row["result"], "API 准备发布结果")?;
        if result["handoff"] == handoff {
            prepared = Some(result);
            break;
        }
    }
    let prepared = prepared.ok_or("活动登记没有相同运行位置的已发布 prepare")?;
    let current = common::read_bound(context.backend, &handoff, "活动 API 交接")?;
    if prepared["status"] != "post_copy_api_prepared"
        || prepared["worker_started"] != false
        || current["registration"] != context.post.descriptor
        || current["worker_started"] != false
        || current["api_url"] != context.selected["api_url"]
        || common::bound_file(context.backend, &current["runtime"], "活动运行合同")?
            != directory.join("runtime.json")
        || common::bound_file(context.backend, &current["binaries"], "活动产物绑定")?
            != directory.join("binaries.json")
    {
        return Err("API 交接不属于活动登记、准备结果或停止的 Worker".into());
    }
    let runtime = runtime::contract(context.backend, product, directory)?;
    let api_path = common::common_file(
        context.backend,
        &directory.join("api.json"),
        "API 原进程收据",
    )?;
    let api = document::read(&api_path)?;
    document::exact(
        &api,
        &["format_version", "role", "scope_id", "identity"],
        "API 原进程收据",
    )?;
    if api["format_version"] != 1
        || api["role"] != "api"
        || api["scope_id"] != context.selected["scope_id"]
        || runtime["scope_id"] != context.selected["scope_id"]
        || runtime["worker_ready_url"] != context.selected["worker_ready_url"]
        || api["identity"]["executable"] != runtime["artifacts"]["api"]["path"]
    {
        return Err("活动 API 原进程、运行合同或角色地址不同于登记".into());
    }
    let build = common::read_bound(
        context.backend,
        &context.post.request["backend_build"],
        "API/Worker 原产品构建",
    )?;
    let mut paths = vec![
        api_path.clone(),
        directory.join("runtime.json"),
        directory.join("binaries.json"),
        directory.join("handoff.json"),
    ];
    for (role, key) in [("api", "api_config_dir"), ("worker", "worker_config_dir")] {
        let artifact = &build["artifacts"][role];
        if runtime["artifacts"][role]
            != json!({"path":artifact["executable"],"sha256":artifact["sha256"]})
        {
            return Err("活动运行合同使用其他产品构建".into());
        }
        let config = Path::new(document::text(&runtime[key], "原角色配置目录")?);
        paths.extend(
            ["app.toml", "app.test.toml"]
                .into_iter()
                .map(|name| config.join(name))
                .filter(|path| path.exists()),
        );
    }
    let files = Inputs::open(paths)?;
    if document::read(&api_path)? != api
        || runtime::contract(context.backend, product, directory)? != runtime
        || common::binding(&directory.join("handoff.json"))? != handoff
    {
        return Err("活动 API 运行合同在取得保护前变化".into());
    }
    files.check()?;
    Ok((
        runtime,
        common::validate_process_identity(&api["identity"])?,
        files,
    ))
}

fn process_id(value: &Value) -> Result<u32> {
    Ok(u32::try_from(
        value["pid"].as_u64().ok_or("活动 API PID 无效")?,
    )?)
}

#[cfg(test)]
mod tests {
    use super::RoleWindow;

    #[test]
    fn only_seed_lifecycle_allows_the_new_worker_to_occupy_its_port() {
        use RoleWindow::{ActiveApi, SeedPrepare, SeedRuntime};
        assert!(RoleWindow::select("seed-runtime", "start") == SeedRuntime);
        assert!(RoleWindow::select("seed-runtime", "close") == SeedRuntime);
        assert!(RoleWindow::select("seed-runtime", "prepare") == SeedPrepare);
        assert!(RoleWindow::select("seed-runtime", "departments-apply") == ActiveApi);
        assert!(RoleWindow::select("post-copy", "prepare") == ActiveApi);
    }
}

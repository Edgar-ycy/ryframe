//! API 交接只读守卫共用已登记物理配置、存储代次与固定数据库查询。
use super::input::Frame;
use crate::{
    Result,
    recovery::{
        clone::native::{
            bridge::scope::Scope,
            digest, export,
            input::BUCKETS,
            json as document,
            lifecycle::manifest::fixture_storage,
            resources::common,
            s3::{Operation, app_configuration},
            target_verify::{self, guard::Generation, storage::StorageInputs},
        },
        full_stack::worker::{self, role::Role},
    },
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(super) fn before(
    frame: &Frame,
    owner: &Generation,
    scope: &Scope,
    storage: &Path,
) -> Result<()> {
    frame.check(owner, scope)?;
    target(frame, owner, storage, false)?;
    frame.check(owner, scope)
}

pub(super) fn after(
    frame: &Frame,
    owner: &Generation,
    scope: &Scope,
    storage: &Path,
) -> Result<()> {
    frame.check(owner, scope)?;
    if export::proof::generation_in(&frame.source, scope)? != frame.source_generation {
        return Err("停止的原来源代次在 API 交接期间变化".into());
    }
    let source = &frame.source.request["source"];
    let runtime = common::read_bound(
        &frame.backend,
        &frame.source.request["runtime"],
        "原来源运行配置",
    )?;
    let configuration = app_configuration::configuration_value(
        &frame.source.execution_backend,
        Path::new(document::text(
            &runtime["api_config_dir"],
            "原来源配置目录",
        )?),
        "test",
    )?;
    let request = json!({"target":source,"tools":frame.source.request["tools"],
        "execution_backend":{"path":frame.source.execution_backend}});
    let inputs = StorageInputs {
        backend: &frame.backend,
        request: &request,
        environment: &frame.source.environment,
        observation_dir: &frame.output,
    };
    databases(&inputs, &configuration, || {
        owner.checkpoint()?;
        frame.protected.checkpoint()
    })?;
    target(frame, owner, storage, true)?;
    frame.check(owner, scope)
}

fn target(frame: &Frame, owner: &Generation, storage: &Path, running: bool) -> Result<()> {
    let fixture = arm_storage(frame, owner, storage)?;
    let result = target_current(frame, owner, storage, running, fixture.as_ref());
    super::locks::combine(
        result,
        fixture.map_or(Ok(()), |fixture| fixture.check(&frame.backend)),
    )
}

fn target_current(
    frame: &Frame,
    owner: &Generation,
    storage: &Path,
    running: bool,
    fixture: Option<&fixture_storage::Fixture>,
) -> Result<()> {
    let environment: BTreeMap<String, String> = serde_json::from_value(
        common::read_bound(
            &frame.backend,
            &frame.post.request["api_environment"],
            "API冻结环境",
        )?["environment"]
            .clone(),
    )
    .map_err(|_| "API冻结环境必须是文本映射")?;
    let inputs = StorageInputs {
        backend: &frame.backend,
        request: &frame.target,
        environment: &environment,
        observation_dir: &frame.output,
    };
    let root = inputs.execution_backend()?;
    let (directory, files) = target_verify::input::request_configuration::fresh_files(
        &root,
        &environment,
        &frame.target,
    )?;
    let configuration = app_configuration::configuration_value(&root, &directory, "test")?;
    let (api_directory, api_configuration) =
        api_configuration_binding(&frame.backend, &frame.runtime)?;
    let mut declared = frame.target["target"].clone();
    declared["s3"] = json!({"configuration":api_configuration});
    if crate::recovery::source::evidence::binding::physical(&json!({"backend":root,
        "evidence_root":frame.backend,"source":declared,"app_env":"test","config_dir":api_directory}))?
        != frame.initial["generation"]["physical"]
    {
        return Err("交接API物理目标不同于原复制目标".into());
    }
    addresses(frame, api_directory)?;
    absent(&frame.post.runtime.join("worker.json"))?;
    absent(Path::new(document::text(
        &frame.selected["runtime_dir"],
        "原目标运行目录",
    )?))?;
    verify_target_storage_layout(frame, owner, storage, fixture)?;
    databases(&inputs, &configuration, || {
        owner.checkpoint()?;
        frame.protected.checkpoint()
    })?;
    objects(&inputs)?;
    if running {
        api(frame)?;
    } else {
        common::require_closed_port(document::text(&frame.selected["api_url"], "目标API端口")?)?;
        if worker::controller::observe(Role::Api, &frame.product, &frame.post.runtime)?["state"]
            != "stopped"
        {
            return Err("已登记API树仍运行，拒绝重复启动".into());
        }
    }
    if target_verify::input::request_configuration::fresh_files(&root, &environment, &frame.target)?
        != (directory, files)
    {
        return Err("API交接期间原目标文件配置变化".into());
    }
    Ok(())
}

pub(super) fn api_configuration_binding<'a>(
    evidence_root: &Path,
    runtime: &'a Value,
) -> Result<(&'a Path, Value)> {
    let directory = Path::new(document::text(&runtime["api_config_dir"], "API配置目录")?);
    let binding = app_configuration::configuration_value(evidence_root, directory, "test")?;
    Ok((directory, binding))
}

fn verify_target_storage_layout(
    frame: &Frame,
    owner: &Generation,
    storage: &Path,
    fixture: Option<&fixture_storage::Fixture>,
) -> Result<()> {
    let (selected, cache) = if let Some(fixture) = fixture {
        fixture.check(&frame.backend)?;
        let projection = target_verify::storage::arm_fixture_projection(
            &frame.backend,
            &fixture.run,
            &frame.target["storage"],
        )?;
        fixture.check(&frame.backend)?;
        projection
    } else {
        target_verify::fresh::selection::transition(
            &frame.backend,
            &frame.target,
            Some(storage),
            Some(owner),
        )?
    };
    target_verify::storage::protocol::execute(
        &frame.backend,
        &frame.output,
        "storage-layout",
        &json!({"request":frame.target_descriptor,"storage":selected.value,
        "runtime":selected.runtime,"cache_runtime":cache}),
    )?;
    Ok(())
}

fn arm_storage(
    frame: &Frame,
    owner: &Generation,
    storage: &Path,
) -> Result<Option<fixture_storage::Fixture>> {
    if frame.manifest["copy_stage"] != "seed_to_arm" {
        return Ok(None);
    }
    let expected = &frame.manifest["target_storage_run"];
    if let Some(fixture) = fixture_storage::resolve(
        &frame.backend,
        &frame.manifest["target_registration"],
        Some(expected),
        true,
    )? {
        if fixture.run != storage {
            return Err("API交接夹具代次不属于原运行目录".into());
        }
        return Ok(Some(fixture));
    }
    let actual = crate::recovery::clone::native::resources::storage::registered::registered(
        &frame.backend,
        storage,
        crate::cli::CloneSide::Target,
        None,
    )?;
    if actual.as_ref() != Some(&expected["storage"])
        || super::super::binding::load(&frame.backend, storage, Some(owner))? != expected["cache"]
    {
        return Err("API交接目标存储完整登记代次不同于原复制清单".into());
    }
    Ok(None)
}

fn addresses(frame: &Frame, directory: &Path) -> Result<()> {
    let table = serde_json::to_value(
        crate::rate_limit::load_app_table(&frame.product, Some(directory), "test")
            .map_err(|_| "无法安全读取已绑定API配置")?,
    )?;
    export::proof::verify_api_address(
        document::text(&frame.selected["api_url"], "API地址")?,
        &table,
    )?;
    export::proof::verify_worker_address(
        document::text(&frame.selected["worker_ready_url"], "Worker地址")?,
        &table,
    )?;
    if frame.runtime["worker_ready_url"] != frame.selected["worker_ready_url"]
        || frame.runtime["scope_id"] != frame.target["target"]["scope_id"]
    {
        return Err("API交接运行scope或Worker端口变化".into());
    }
    target_verify::authority::require_closed(document::text(
        &frame.selected["worker_ready_url"],
        "Worker就绪地址",
    )?)
}

fn databases(
    inputs: &StorageInputs<'_>,
    configuration: &Value,
    checkpoint: impl Fn() -> Result<()>,
) -> Result<()> {
    let transport = export::transport::SystemTransport;
    for database in inputs.databases()? {
        let actual = target_verify::database::evidence::verify_query(
            inputs,
            configuration,
            &transport,
            &checkpoint,
            &database,
            "identity",
        )?;
        if actual.split('\t').collect::<Vec<_>>()
            != [
                document::text(&database["server_uuid"], "数据库UUID")?,
                document::text(&database["database"], "物理库名")?,
            ]
        {
            return Err("API交接数据库实际UUID或库名不同于原登记".into());
        }
        let rows = target_verify::database::evidence::verify_query(
            inputs,
            configuration,
            &transport,
            &checkpoint,
            &database,
            "ownership",
        )?;
        let scope = inputs.target_scope()?;
        let kinds = if database["kind"] == "combined" {
            vec!["control", "tenant-data"]
        } else {
            vec!["tenant-data"]
        };
        let expected = kinds
            .into_iter()
            .map(|kind| format!("{kind}\t{scope}\tryframe-owner:v1:{scope}:{kind}"))
            .collect::<BTreeSet<_>>();
        if rows.lines().map(str::to_owned).collect::<BTreeSet<_>>() != expected {
            return Err("API交接数据库完整ownership不同于原登记".into());
        }
    }
    Ok(())
}

fn objects(inputs: &StorageInputs<'_>) -> Result<()> {
    let scope = inputs.target_scope()?;
    for bucket in BUCKETS {
        let path = inputs.observation_dir.join(format!(
            "api-owner-{bucket}-{}.bin",
            uuid::Uuid::new_v4().simple()
        ));
        target_verify::fresh::objects::observe(
            inputs,
            &export::transport::SystemTransport,
            bucket,
            Operation::Get {
                key: format!("{scope}/.ryframe-owner"),
                if_match: None,
                version: None,
                destination: path.clone(),
                max_bytes: 4096,
            },
            "api-owner",
            None,
        )?;
        if std::fs::read(common::common_file(
            inputs.backend,
            &path,
            "API对象owner正文",
        )?)? != format!("ryframe-owner:v1:{scope}:object-storage:{bucket}").as_bytes()
        {
            return Err("API交接对象owner正文不同于固定scope".into());
        }
    }
    Ok(())
}

fn api(frame: &Frame) -> Result<()> {
    live_api(
        &frame.backend,
        &frame.product,
        &frame.post.runtime,
        &frame.runtime,
    )
    .map(|_| ())
}

pub(super) fn live_api(
    backend: &Path,
    product: &Path,
    directory: &Path,
    runtime: &Value,
) -> Result<Value> {
    let observed = worker::controller::observe(Role::Api, product, directory)?;
    if observed["state"] != "running"
        || observed["identity"]["executable"] != runtime["artifacts"]["api"]["path"]
    {
        return Err("API实际内核身份、完整监督树或产物不同于冻结交接".into());
    }
    let actual = target_verify::storage::protocol::arguments(
        backend,
        directory,
        &common::binding(&directory.join("api.json"))?,
        &observed["identity"],
    )?;
    let expected = json!([
        runtime["artifacts"]["api"]["path"],
        "--config-dir",
        runtime["api_config_dir"]
    ]);
    if actual["actual_arguments_sha256"] != digest::plan_hash(&expected)? {
        return Err("API完整实际参数不同于冻结产物和明确配置目录".into());
    }
    Ok(observed)
}

pub(super) fn absent(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("未登记生产者、旧运行目录或控制锁仍存在；保留现场，不自动接管".into()),
    }
}

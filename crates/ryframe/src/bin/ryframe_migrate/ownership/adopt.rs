use ryframe_config::{AppConfig, Environment, TenantDatabaseTargetKind};
use sea_orm::{ConnectionTrait, DatabaseConnection, DatabaseTransaction};
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use super::{
    guard::Guard,
    state::{self, Snapshot},
};
#[path = "cas.rs"]
mod cas;

pub(crate) struct Options {
    kind: String,
    target: Option<String>,
    action: Action,
    plan: bool,
}

enum Action {
    Preview(Option<PathBuf>),
    Apply(PathBuf),
    Inspect(PathBuf),
}

pub(crate) fn parse(args: &[String]) -> Result<Options, Box<dyn Error>> {
    let mut target = None;
    let mut output = None;
    let mut apply = None;
    let mut inspect = None;
    let mut write = false;
    let mut plan = false;
    let mut values = args[2..].iter();
    while let Some(flag) = values.next() {
        match flag.as_str() {
            "--write" if !write => write = true,
            "--plan" if !plan => plan = true,
            "--target" | "--output" | "--apply" | "--inspect" => {
                let value = values
                    .next()
                    .filter(|v| !v.is_empty() && !v.starts_with("--"))
                    .ok_or("ownership-adopt 参数缺少值")?
                    .clone();
                let slot = match flag.as_str() {
                    "--target" => &mut target,
                    "--output" => &mut output,
                    "--apply" => &mut apply,
                    _ => &mut inspect,
                };
                if slot.replace(value).is_some() {
                    return Err("重复 ownership-adopt 参数".into());
                }
            }
            _ => return Err("未知 ownership-adopt 参数；不能混用业务 --module adopt".into()),
        }
    }
    let kind = args.first().ok_or("缺少资源范围")?.clone();
    if !matches!(kind.as_str(), "control" | "tenant-data")
        || (kind == "control") == target.is_some()
        || (plan && write)
    {
        return Err("control 不接受 target；tenant-data 必须指定 target；plan 不接受 write".into());
    }
    let action = match (output, apply, inspect) {
        (output, None, None) if plan || write == output.is_some() => {
            Action::Preview(output.map(PathBuf::from))
        }
        (None, Some(path), None) if write || plan => Action::Apply(path.into()),
        (None, None, Some(path)) if !write => Action::Inspect(path.into()),
        _ => {
            return Err(
                "预览保存用 --output --write；应用用 --apply --write；只读核对用 --inspect".into(),
            );
        }
    };
    Ok(Options {
        kind,
        target,
        action,
        plan,
    })
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u16,
    scope_id: String,
    resource_kind: String,
    target: Option<String>,
    physical_target: (String, String),
    primary_target: (String, String),
    configuration: Vec<(String, String)>,
    source: String,
    primary_fences: String,
    producers_stopped_declaration: bool,
    before: Snapshot,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    manifest: Manifest,
    fingerprint: String,
}

pub(crate) async fn run(directory: &Path, options: Options) -> Result<(), Box<dyn Error>> {
    if options.plan {
        println!(
            "ownership-adopt：读取原计划→配置/业务登记→明确物理目标→结构/账本/fence→完整前后像CAS→归属登记；当前任务图未读取文件或数据库，未写入"
        );
        return Ok(());
    }
    let saved = match &options.action {
        Action::Apply(path) | Action::Inspect(path) => {
            let path = super::super::private_artifact::input(path)?;
            if fs::metadata(&path)?.len() > 1_048_576 {
                return Err("归属计划超过1MiB".into());
            }
            Some(serde_json::from_slice::<Plan>(&fs::read(path)?)?)
        }
        Action::Preview(_) => None,
    };
    let environment = Environment::from_env()?;
    let files = configuration(directory, environment)?;
    let config = AppConfig::load(directory, environment)?;
    if let Some(plan) = &saved
        && (plan.manifest.version != 1
            || plan.fingerprint != state::digest(&serde_json::to_vec(&plan.manifest)?)
            || plan.manifest.scope_id != config.scope_id.as_str()
            || plan.manifest.resource_kind != options.kind
            || plan.manifest.target != options.target)
    {
        return Err("原归属计划版本、摘要、scope 或目标与命令不一致，尚未连接数据库".into());
    }
    if !config.reset.services_stopped {
        return Err("ownership-adopt 必须先停止全部生产者并在绑定配置显式声明 reset.services_stopped=true；该声明不等于进程停止证明".into());
    }
    let modules = ryframe::business::registered_business_modules()?;
    let registry = ryframe_tenant_db::TenantDatabaseTargetRegistry::new(
        &config.tenant_data,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )?;
    let lease = match options.target.as_deref() {
        Some(key) => match registry.target_kind(key) {
            Some(TenantDatabaseTargetKind::Mysql) => Some(registry.acquire(key).await?),
            Some(TenantDatabaseTargetKind::Control) => None,
            None => return Err("接管目标未登记".into()),
        },
        None => None,
    };
    let primary = ryframe_db::connection::connect_with_sql_logging(
        &config.database.primary,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )
    .await?;
    let db = lease.as_ref().map_or(&primary, |lease| lease.connection());
    let result = execute(
        db,
        (&primary, directory, environment, &files),
        &config,
        &options,
        saved.as_ref(),
        &modules,
        lease.is_none(),
    )
    .await;
    drop(lease);
    let closed = primary.close().await;
    result?;
    closed?;
    Ok(())
}

async fn execute(
    db: &DatabaseConnection,
    inputs: (&DatabaseConnection, &Path, Environment, &[(String, String)]),
    config: &AppConfig,
    options: &Options,
    saved: Option<&Plan>,
    modules: &[ryframe_sdk::RyFrameBusinessModule],
    shared: bool,
) -> Result<(), Box<dyn Error>> {
    let mut guard = Guard::acquire(db).await?;
    let result = if shared {
        perform(
            guard.session_mut(),
            (None::<&DatabaseConnection>, inputs.1, inputs.2, inputs.3),
            config,
            options,
            saved,
            modules,
            shared,
        )
        .await
    } else {
        perform(
            guard.session_mut(),
            (Some(inputs.0), inputs.1, inputs.2, inputs.3),
            config,
            options,
            saved,
            modules,
            shared,
        )
        .await
    };
    let closed = guard.close().await;
    match (result, closed) {
        (Err(error), Err(close)) => {
            Err(format!("{error}；归属会话清理也失败：{close}；先 inspect 原计划").into())
        }
        (Err(error), _) => Err(error),
        (_, Err(error)) => Err(error.into()),
        _ => Ok(()),
    }
}

async fn perform<P: ConnectionTrait>(
    db: &mut DatabaseTransaction,
    inputs: (Option<&P>, &Path, Environment, &[(String, String)]),
    config: &AppConfig,
    options: &Options,
    saved: Option<&Plan>,
    modules: &[ryframe_sdk::RyFrameBusinessModule],
    shared: bool,
) -> Result<(), Box<dyn Error>> {
    let (_, directory, environment, files) = inputs;
    async {
        cas::verify(db, &options.kind, modules, shared).await?;
        let current = current_manifest(
            db,
            inputs,
            config,
            options,
            modules,
            &ryframe_db::connection::physical_identity(db).await?,
        )
        .await?;
        if configuration(directory, environment)? != files {
            return Err("配置读取期间发生漂移".into());
        }
        match &options.action {
            Action::Preview(output) => {
                cas::validate_before(&current)?;
                let plan = Plan {
                    fingerprint: state::digest(&serde_json::to_vec(&current)?),
                    manifest: current,
                };
                let bytes = serde_json::to_vec_pretty(&plan)?;
                if let Some(path) = output {
                    let path = super::super::private_artifact::output(path)?;
                    super::super::private_artifact::publish(&path, &bytes)?;
                } else {
                    println!("{}", String::from_utf8(bytes)?);
                }
            }
            Action::Apply(_) | Action::Inspect(_) => {
                let plan = saved.ok_or("缺少原计划")?;
                let applied = cas::inspect(plan, &current)?;
                if matches!(options.action, Action::Apply(_)) && !applied {
                    let identity = ryframe_db::connection::physical_identity(db).await?;
                    cas::apply(
                        db,
                        plan,
                        &identity,
                        inputs,
                        config,
                        options.target.as_deref(),
                    )
                    .await?;
                }
                println!(
                    "ownership-adopt state={}",
                    if applied || matches!(options.action, Action::Apply(_)) {
                        "applied"
                    } else {
                        "unapplied"
                    }
                );
            }
        }
        Ok::<_, Box<dyn Error>>(())
    }
    .await
}

fn configuration(
    directory: &Path,
    environment: Environment,
) -> Result<Vec<(String, String)>, Box<dyn Error>> {
    let mut files = Vec::new();
    for name in [
        "app.toml".to_owned(),
        format!("app.{}.toml", environment.as_str()),
    ] {
        let path = directory.join(&name);
        if name == "app.toml" && !path.exists() {
            continue;
        }
        let path = path.canonicalize()?;
        files.push((
            path.to_string_lossy().into_owned(),
            state::digest(&fs::read(path)?),
        ));
    }
    Ok(files)
}

async fn current_manifest<P: ConnectionTrait>(
    db: &DatabaseTransaction,
    inputs: (Option<&P>, &Path, Environment, &[(String, String)]),
    config: &AppConfig,
    options: &Options,
    modules: &[ryframe_sdk::RyFrameBusinessModule],
    identity: &ryframe_db::connection::DatabaseIdentity,
) -> Result<Manifest, Box<dyn Error>> {
    let (primary, _, _, files) = inputs;
    let before = state::snapshot(db).await?;
    let current = Manifest {
        version: 1,
        scope_id: config.scope_id.as_str().to_owned(),
        resource_kind: options.kind.clone(),
        target: options.target.clone(),
        physical_target: (identity.server_uuid.clone(), identity.database.clone()),
        primary_target: {
            let identity = match primary {
                Some(primary) => ryframe_db::connection::physical_identity(primary).await?,
                None => identity.clone(),
            };
            (identity.server_uuid, identity.database)
        },
        configuration: files.to_vec(),
        source: super::contract::fingerprint(config.scope_id.as_str(), modules)?,
        primary_fences: match primary {
            Some(primary) => {
                super::fence::verify(db, primary, config, options.target.as_deref()).await?
            }
            None => super::fence::verify(db, db, config, options.target.as_deref()).await?,
        },
        producers_stopped_declaration: true,
        before,
    };

    Ok(current)
}

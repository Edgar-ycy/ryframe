use std::{error::Error, path::Path};

use ryframe_config::{AppConfig, Environment, TenantDatabaseTargetKind};
use ryframe_sdk::{BusinessAdoptionPlan, BusinessMigrationScope, RyFrameBusinessModule};
use ryframe_tenant_db::{TenantDatabasePoolLease, TenantDatabaseTargetRegistry};
use sea_orm::DatabaseConnection;

use super::{Action, Options};

#[derive(Clone, Copy)]
enum Task {
    ReadPlan,
    LoadConfig,
    RegisterModules,
    Connect,
    VerifyBaseline,
    Operation,
    Close,
}

fn tasks(options: &Options) -> Vec<Task> {
    let mut tasks = Vec::new();
    if !matches!(options.action, Action::Preview { .. }) {
        tasks.push(Task::ReadPlan);
    }
    tasks.extend([
        Task::LoadConfig,
        Task::RegisterModules,
        Task::Connect,
        Task::VerifyBaseline,
        Task::Operation,
        Task::Close,
    ]);
    tasks
}

#[derive(Default)]
struct State {
    saved: Option<BusinessAdoptionPlan>,
    config: Option<AppConfig>,
    modules: Vec<RyFrameBusinessModule>,
    connection: Option<Connection>,
}

enum Connection {
    Primary(DatabaseConnection),
    Tenant(TenantDatabasePoolLease),
}

impl Connection {
    fn database(&self) -> &DatabaseConnection {
        match self {
            Self::Primary(database) => database,
            Self::Tenant(lease) => lease.connection(),
        }
    }

    async fn close(self) -> Result<(), Box<dyn Error>> {
        match self {
            Self::Primary(database) => database.close().await?,
            Self::Tenant(lease) => lease.connection().clone().close().await?,
        }
        Ok(())
    }
}

pub(super) async fn run(directory: &Path, options: Options) -> Result<(), Box<dyn Error>> {
    let tasks = tasks(&options);
    if options.plan {
        print_plan(&options, &tasks);
        return Ok(());
    }
    let mut state = State::default();
    for task in tasks {
        if let Err(error) = execute(task, directory, &options, &mut state).await {
            if let Some(connection) = state.connection.take()
                && let Err(close) = connection.close().await
            {
                return Err(
                    format!("{error}；关闭目标连接时另有错误：{close}，保留现场先核对").into(),
                );
            }
            return Err(error);
        }
    }
    Ok(())
}

async fn execute(
    task: Task,
    directory: &Path,
    options: &Options,
    state: &mut State,
) -> Result<(), Box<dyn Error>> {
    match task {
        Task::ReadPlan => {
            let path = match &options.action {
                Action::Apply(path) | Action::Inspect(path) => path,
                Action::Preview { .. } => return Err("预览任务图不能读取原计划".into()),
            };
            let saved = super::read_plan(path)?;
            if saved.manifest.scope != scope(options) {
                return Err(
                    "原计划范围与命令不同，不能跨 control/tenant 接管，尚未连接数据库".into(),
                );
            }
            state.saved = Some(saved);
        }
        Task::LoadConfig => {
            state.config = Some(AppConfig::load(directory, Environment::from_env()?)?)
        }
        Task::RegisterModules => state.modules = ryframe::business::registered_business_modules()?,
        Task::Connect => {
            state.connection = Some(
                connect(
                    options,
                    state.config.as_ref().ok_or("接管任务图缺少配置依赖")?,
                )
                .await?,
            )
        }
        Task::VerifyBaseline => {
            let connection = state.connection.as_ref().ok_or("接管任务图缺少连接依赖")?;
            let database = connection.database();
            match connection {
                Connection::Primary(_) => ryframe_db::migration::verify(database).await?,
                Connection::Tenant(_) => {
                    ryframe_tenant_db::migration::ensure_mysql_target_boundary(database).await?
                }
            }
            if options.target.is_some() {
                ryframe_tenant_db::migration::verify(database).await?;
            }
        }
        Task::Operation => {
            super::perform(
                options,
                state.saved.as_ref(),
                scope(options),
                state
                    .connection
                    .as_ref()
                    .ok_or("接管任务图缺少连接依赖")?
                    .database(),
                &state.modules,
            )
            .await?
        }
        Task::Close => {
            state
                .connection
                .take()
                .ok_or("接管任务图缺少待关闭连接")?
                .close()
                .await?
        }
    }
    Ok(())
}

async fn connect(options: &Options, config: &AppConfig) -> Result<Connection, Box<dyn Error>> {
    if let Some(target) = &options.target {
        let registry = TenantDatabaseTargetRegistry::new(
            &config.tenant_data,
            config.database.sql_log_level,
            config.database.sql_slow_threshold_ms,
        )?;
        match registry.target_kind(target) {
            Some(TenantDatabaseTargetKind::Mysql) => {
                return Ok(Connection::Tenant(registry.acquire(target).await?));
            }
            Some(TenantDatabaseTargetKind::Control) => {}
            None => {
                return Err(format!(
                    "租户数据目标 {target} 未配置；核对 tenant_data.targets[].key，尚未写入"
                )
                .into());
            }
        }
    }
    Ok(Connection::Primary(
        ryframe_db::connection::connect_with_sql_logging(
            &config.database.primary,
            config.database.sql_log_level,
            config.database.sql_slow_threshold_ms,
        )
        .await?,
    ))
}

fn scope(options: &Options) -> BusinessMigrationScope {
    if options.target.is_some() {
        BusinessMigrationScope::Tenant
    } else {
        BusinessMigrationScope::Control
    }
}

fn print_plan(options: &Options, tasks: &[Task]) {
    let target = options.target.as_deref().unwrap_or("control");
    println!(
        "任务图：范围={}，目标={target}，编译覆盖=bin-migrate；各任务依赖前一项：",
        scope(options).as_str()
    );
    for (index, task) in tasks.iter().enumerate() {
        let label = match task {
            Task::ReadPlan => "读取原计划并核对命令范围",
            Task::LoadConfig => "加载明确运行配置与秘密引用",
            Task::RegisterModules => "核对完整注册业务模块与目录",
            Task::Connect => "连接所选明确目标",
            Task::VerifyBaseline => "只读核验框架初始化基线与目标边界",
            Task::Operation => match &options.action {
                Action::Preview { output: None, .. } => "只读生成接管计划并输出 JSON",
                Action::Preview {
                    output: Some(_), ..
                } => "只读生成接管计划并显式保存新文件",
                Action::Apply(_) => "核对物理目标、源码、完整前后像并原子登记 adopted 元数据",
                Action::Inspect(_) => "只读核对完整前后像并输出 unapplied/applied 或冲突",
            },
            Task::Close => "释放连接；失败路径同样释放",
        };
        println!("  {}. {label}", index + 1);
    }
    println!(
        "当前 --plan 未执行任务、未读配置/秘密/文件/数据库，未创建目录、锁或报告；写入操作须 --write。"
    );
}

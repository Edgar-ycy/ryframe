use std::{error::Error, path::Path};

use ryframe_config::{AppConfig, Environment, TenantDatabaseTargetKind};
use ryframe_sdk::{BusinessAdoptionPlan, BusinessMigrationScope, RyFrameBusinessModule};
use ryframe_tenant_db::{TenantDatabasePoolLease, TenantDatabaseTargetRegistry};
use sea_orm::DatabaseConnection;

use super::super::diagnostic::{Context, Writes, finish};
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

impl Task {
    fn label(self) -> &'static str {
        match self {
            Self::ReadPlan => "读取原接管计划",
            Self::LoadConfig => "配置加载",
            Self::RegisterModules => "业务模块登记",
            Self::Connect => "接管目标连接",
            Self::VerifyBaseline => "接管基线只读核验",
            Self::Operation => "接管预览/应用/核对",
            Self::Close => "接管关闭连接",
        }
    }
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
    environment: Option<Environment>,
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
    let mut writes_started = false;
    let action = match &options.action {
        Action::Preview { .. } => "preview",
        Action::Apply(_) => "--apply",
        Action::Inspect(_) => "--inspect",
    };
    let command = format!(
        "{} adopt {action}",
        if options.target.is_some() {
            "tenant-data"
        } else {
            "control"
        }
    );
    for task in tasks {
        if matches!(task, Task::Operation) && matches!(options.action, Action::Apply(_)) {
            writes_started = true;
        }
        if let Err(error) = execute(task, directory, &options, &mut state).await {
            let context = Context {
                directory,
                environment: state.environment,
                config: state.config.as_ref(),
                command: &command,
                target: options.target.as_deref(),
            };
            let writes = if writes_started {
                Writes::Database
            } else if matches!(task, Task::Operation | Task::Close)
                && matches!(
                    options.action,
                    Action::Preview {
                        output: Some(_),
                        ..
                    }
                )
            {
                Writes::File
            } else {
                Writes::None
            };
            let phase = if matches!(task, Task::Operation) {
                match &options.action {
                    Action::Preview { .. } => "接管预览/计划发布",
                    Action::Apply(_) => "接管账本应用",
                    Action::Inspect(_) => "接管计划只读核对",
                }
            } else {
                task.label()
            };
            let failure = context.error(phase, writes, error.as_ref());
            let closed = match state.connection.take() {
                Some(connection) => connection
                    .close()
                    .await
                    .map_err(|error| context.error("接管失败后关闭连接", writes, error.as_ref())),
                None => Ok(()),
            };
            return finish(Err(failure), closed);
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
        Task::ReadPlan => read_plan(options, state)?,
        Task::LoadConfig => {
            let environment = Environment::from_env()
                .map_err(|_| std::io::Error::other("APP_ENV 必须为 dev、test 或 prod"))?;
            state.environment = Some(environment);
            state.config = Some(AppConfig::load(directory, environment)?)
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
                Connection::Primary(_) => {
                    ryframe_db::migration::verify_control_baseline(database).await?
                }
                Connection::Tenant(_) => {
                    ryframe_tenant_db::migration::ensure_mysql_target_boundary(database).await?
                }
            }
            if options.target.is_some() {
                match connection {
                    Connection::Primary(_) => {
                        ryframe_tenant_db::migration::verify_shared_control(database).await?
                    }
                    Connection::Tenant(_) => ryframe_tenant_db::migration::verify(database).await?,
                }
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

fn read_plan(options: &Options, state: &mut State) -> Result<(), Box<dyn Error>> {
    let path = match &options.action {
        Action::Apply(path) | Action::Inspect(path) => path,
        Action::Preview { .. } => return Err("预览任务图不能读取原计划".into()),
    };
    let saved = super::read_plan(path)?;
    if saved.manifest.scope != scope(options) {
        return Err("原计划范围与命令不同，不能跨 control/tenant 接管，尚未连接数据库".into());
    }
    state.saved = Some(saved);
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

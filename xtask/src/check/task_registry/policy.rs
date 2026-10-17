use super::{TaskDefinition, TaskExecutor, TaskRepository, TaskStage, TaskWorkingDirectory};

pub(super) const TASKS: [TaskDefinition; 2] = [
    task_definition!(
        DeploymentAssets,
        "policy.deployment-assets",
        "check_deployment_assets",
        "用 Rust 只读核验生产部署资产、固定镜像和配置约束",
        Backend,
        Static,
        Backend,
        None,
        &[],
        &[],
        &[]
    ),
    task_definition!(
        PermissionRoutes,
        "policy.permission-routes",
        "check_permission_routes",
        "扫描在线生成器残留并构建 ryframe-api 访问策略",
        Backend,
        Static,
        Backend,
        None,
        &["ryframe-api cargo check"],
        &["Cargo target"],
        &[]
    ),
];

use std::collections::BTreeSet;

use crate::Result;

pub(crate) use super::task_registry::{
    TASK_REGISTRY, TaskDefinition, TaskExecutor, TaskRepository, TaskStage, TaskWorkingDirectory,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskSpec {
    pub(crate) id: &'static str,
    pub(crate) dependencies: Vec<&'static str>,
    pub(crate) executor: TaskExecutor,
}

impl TaskSpec {
    pub(crate) fn definition(&self) -> &'static TaskDefinition {
        self.executor.definition()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskPlan {
    pub(crate) tasks: Vec<TaskSpec>,
}

impl TaskPlan {
    pub(crate) fn sequence(executors: &[TaskExecutor]) -> Result<Self> {
        validate_registry()?;
        let mut tasks = Vec::<TaskSpec>::with_capacity(executors.len());
        for executor in executors {
            let definition = executor.definition();
            let dependencies = tasks.last().map(|task| task.id).into_iter().collect();
            tasks.push(TaskSpec {
                id: definition.id,
                dependencies,
                executor: *executor,
            });
        }
        let plan = Self { tasks };
        plan.validate()?;
        Ok(plan)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        validate_task_graph(&self.tasks)
    }
}

fn validate_registry() -> Result<()> {
    let mut ids = BTreeSet::new();
    let mut executors = Vec::new();
    for definition in TASK_REGISTRY {
        if !ids.insert(definition.id) {
            return Err(format!("检查任务注册 ID 重复：{}", definition.id).into());
        }
        if executors.contains(&definition.executor) {
            return Err(format!("检查任务执行器重复登记：{:?}", definition.executor).into());
        }
        executors.push(definition.executor);
    }
    Ok(())
}

fn validate_task_graph(tasks: &[TaskSpec]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for task in tasks {
        if !seen.insert(task.id) {
            return Err(format!("检查任务 ID 重复：{}", task.id).into());
        }
        for dependency in &task.dependencies {
            if !seen.contains(dependency) {
                return Err(format!(
                    "检查任务 {} 的依赖 {} 不存在或未按拓扑顺序声明",
                    task.id, dependency
                )
                .into());
            }
        }
    }
    Ok(())
}

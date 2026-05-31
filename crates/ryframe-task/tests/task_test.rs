//! ryframe-task 单元测试
//!
//! 测试 TaskScheduler 的任务注册、暂停/恢复、注销、列表、CRON 更新等核心逻辑。

use std::sync::Arc;

use async_trait::async_trait;
use ryframe_common::AppResult;
use ryframe_task::{ScheduledTask, TaskContext, TaskScheduler};

// ==================== Mock 任务 ====================

struct TestTask {
    name: &'static str,
    cron: &'static str,
}

#[async_trait]
impl ScheduledTask for TestTask {
    fn name(&self) -> &str {
        self.name
    }
    fn cron(&self) -> &str {
        self.cron
    }
    fn description(&self) -> &str {
        "测试任务"
    }
    async fn execute(&self, _ctx: &TaskContext) -> AppResult<String> {
        Ok(format!("{} 执行成功", self.name))
    }
}

// ==================== 辅助函数 ====================

fn make_ctx() -> TaskContext {
    // 测试用空连接，实际 execute 不会被调用
    let db = sea_orm::DatabaseConnection::default();
    TaskContext { db: Arc::new(db) }
}

fn make_scheduler() -> TaskScheduler {
    TaskScheduler::new(make_ctx())
}

// ==================== 任务注册测试 ====================

#[tokio::test]
async fn test_register_task() {
    let scheduler = make_scheduler();
    let task = Arc::new(TestTask {
        name: "hello",
        cron: "0 */5 * * * *",
    });

    scheduler.register(task.clone(), None, false).await.unwrap();
    assert!(scheduler.is_registered("hello").await);
}

#[tokio::test]
async fn test_register_duplicate_overwrites() {
    let scheduler = make_scheduler();
    let task1 = Arc::new(TestTask {
        name: "dup",
        cron: "0 */5 * * * *",
    });
    let task2 = Arc::new(TestTask {
        name: "dup",
        cron: "0 0 * * * *",
    });

    scheduler.register(task1, None, false).await.unwrap();
    scheduler.register(task2, None, false).await.unwrap();
    assert!(scheduler.is_registered("dup").await);

    // 第二次注册应覆盖（HashMap insert）
    let list = scheduler.list().await;
    assert_eq!(list.len(), 1);
}

#[tokio::test]
async fn test_register_task_paused() {
    let scheduler = make_scheduler();
    let task = Arc::new(TestTask {
        name: "paused_job",
        cron: "* * * * * *",
    });

    scheduler.register(task.clone(), None, true).await.unwrap();

    let list = scheduler.list().await;
    let info = list.iter().find(|t| t.name == "paused_job").unwrap();
    assert!(info.paused);
}

#[tokio::test]
async fn test_register_task_cron_override() {
    let scheduler = make_scheduler();
    let task = Arc::new(TestTask {
        name: "override",
        cron: "* * * * * *",
    });

    // 用自定义 cron 覆盖任务默认值
    scheduler
        .register(task.clone(), Some("0 0 0 * * *"), false)
        .await
        .unwrap();

    let list = scheduler.list().await;
    let info = list.iter().find(|t| t.name == "override").unwrap();
    assert_eq!(info.cron, "0 0 0 * * *");
}

#[tokio::test]
async fn test_register_invalid_cron_rejected() {
    let scheduler = make_scheduler();
    let task = Arc::new(TestTask {
        name: "bad",
        cron: "not-a-cron",
    });

    let err = scheduler.register(task, None, false).await.unwrap_err();
    assert!(err.to_string().contains("无效的 cron"));
    assert!(!scheduler.is_registered("bad").await);
}

// ==================== 暂停/恢复测试 ====================

#[tokio::test]
async fn test_pause_resume_task() {
    let scheduler = make_scheduler();
    let task = Arc::new(TestTask {
        name: "toggle",
        cron: "* * * * * *",
    });

    scheduler.register(task.clone(), None, false).await.unwrap();

    // 暂停
    scheduler.pause("toggle").await.unwrap();
    let info = scheduler
        .list()
        .await
        .into_iter()
        .find(|t| t.name == "toggle")
        .unwrap();
    assert!(info.paused);

    // 恢复
    scheduler.resume("toggle").await.unwrap();
    let info = scheduler
        .list()
        .await
        .into_iter()
        .find(|t| t.name == "toggle")
        .unwrap();
    assert!(!info.paused);
}

#[tokio::test]
async fn test_pause_nonexistent_task() {
    let scheduler = make_scheduler();
    let err = scheduler.pause("ghost").await.unwrap_err();
    assert!(err.to_string().contains("不存在"));
}

#[tokio::test]
async fn test_resume_nonexistent_task() {
    let scheduler = make_scheduler();
    let err = scheduler.resume("ghost").await.unwrap_err();
    assert!(err.to_string().contains("不存在"));
}

// ==================== 注销测试 ====================

#[tokio::test]
async fn test_unregister_task() {
    let scheduler = make_scheduler();
    let task = Arc::new(TestTask {
        name: "temp",
        cron: "* * * * * *",
    });

    scheduler.register(task.clone(), None, false).await.unwrap();
    assert!(scheduler.is_registered("temp").await);

    scheduler.unregister("temp").await.unwrap();
    assert!(!scheduler.is_registered("temp").await);
}

#[tokio::test]
async fn test_unregister_nonexistent() {
    let scheduler = make_scheduler();
    let err = scheduler.unregister("ghost").await.unwrap_err();
    assert!(err.to_string().contains("不存在"));
}

// ==================== 列表测试 ====================

#[tokio::test]
async fn test_list_multiple_tasks() {
    let scheduler = make_scheduler();

    let t1 = Arc::new(TestTask {
        name: "task_a",
        cron: "0 */10 * * * *",
    });
    let t2 = Arc::new(TestTask {
        name: "task_b",
        cron: "0 0 * * * *",
    });
    let t3 = Arc::new(TestTask {
        name: "task_c",
        cron: "30 */2 * * * *",
    });

    scheduler.register(t1, None, false).await.unwrap();
    scheduler.register(t2, None, false).await.unwrap();
    scheduler.register(t3, None, true).await.unwrap();

    let list = scheduler.list().await;
    assert_eq!(list.len(), 3);

    // task_c 应为暂停状态
    let c = list.iter().find(|t| t.name == "task_c").unwrap();
    assert!(c.paused);
}

// ==================== Cron 更新测试 ====================

#[tokio::test]
async fn test_update_cron() {
    let scheduler = make_scheduler();
    let task = Arc::new(TestTask {
        name: "cron_test",
        cron: "* * * * * *",
    });

    scheduler.register(task, None, false).await.unwrap();

    scheduler
        .update_cron("cron_test", "0 0 12 * * *")
        .await
        .unwrap();
    let info = scheduler
        .list()
        .await
        .into_iter()
        .find(|t| t.name == "cron_test")
        .unwrap();
    assert_eq!(info.cron, "0 0 12 * * *");
}

#[tokio::test]
async fn test_update_cron_invalid() {
    let scheduler = make_scheduler();
    let task = Arc::new(TestTask {
        name: "cron_bad",
        cron: "* * * * * *",
    });

    scheduler.register(task, None, false).await.unwrap();
    let err = scheduler
        .update_cron("cron_bad", "bad expr")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("无效的 cron"));
}

#[tokio::test]
async fn test_update_cron_nonexistent() {
    let scheduler = make_scheduler();
    let err = scheduler
        .update_cron("ghost", "* * * * * *")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("不存在"));
}

// ==================== TaskContext 测试 ====================

#[tokio::test]
async fn test_task_context_creation() {
    let ctx = make_ctx();
    let _cloned = ctx.clone();
}

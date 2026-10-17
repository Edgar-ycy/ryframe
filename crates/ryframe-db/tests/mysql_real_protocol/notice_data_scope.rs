use std::{sync::Arc, time::Duration};

use ryframe_application::generated::notice::{
    NoticeListParams, NoticeService, UpdateNoticeCommand,
};
use ryframe_db::{ControlDatabaseCluster, generated, migration};
use ryframe_kernel::{ActorContext, AppError, DataScope, PaginationPolicy, ValidatedPageQuery};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};

use super::{bootstrap_hashes, db_error, execute, run_mysql_test};

fn actor(scope: DataScope) -> ActorContext {
    ActorContext {
        user_id: 90021,
        tenant_id: "system".into(),
        username: "scope-fixture-a".into(),
        dept_id: Some(90011),
        dept_path: None,
        data_scope: scope,
        custom_dept_ids: vec![90013],
        include_self: true,
        data_scope_version: 1,
        is_super_admin: false,
    }
}

fn command() -> UpdateNoticeCommand {
    UpdateNoticeCommand {
        title: "Scope-fixture-updated".into(),
        content_markdown: "更新".into(),
        notice_type: None,
        status: "0".into(),
    }
}

fn list() -> NoticeListParams {
    NoticeListParams {
        page: ValidatedPageQuery::new(1, 10, PaginationPolicy::new(10, 100)).unwrap(),
        title: Some("Scope-fixture-".into()),
        notice_type: None,
        status: None,
    }
}

async fn prepare(database: &DatabaseConnection) -> Result<Arc<NoticeService>, String> {
    migration::up(database, Some(&bootstrap_hashes()))
        .await
        .map_err(db_error)?;
    execute(database, "INSERT INTO sys_dept(id,tenant_id,name,parent_id,ancestors) VALUES (90011,'system','Scope-root',NULL,''),(90012,'system','Scope-child',90011,'90011'),(90013,'system','Scope-other',NULL,'')").await?;
    for (user, department) in [(90021, 90011), (90022, 90012), (90023, 90013)] {
        execute(database, &format!("INSERT INTO sys_user(id,tenant_id,username,password_hash,nickname,email,phone,status,dept_id) SELECT {user},'system','scope-fixture-{user}',password_hash,'Scope','','','1',{department} FROM sys_user WHERE id=1")).await?;
    }
    execute(database, "INSERT INTO sys_tenant(id,tenant_id,name,status) VALUES(90030,'scope-other','Scope-other','enabled')").await?;
    execute(database, "INSERT INTO sys_notice(id,tenant_id,title,content,status,created_by,del_flag,created_at,updated_at) VALUES(91999,'scope-other','Scope-fixture-other','其他租户','1',90021,'0','2026-08-20 00:00:00','2026-08-20 00:00:00')").await?;
    for (id, owner) in [
        (91001, "90021"),
        (91002, "90022"),
        (91003, "90023"),
        (91004, "NULL"),
    ] {
        execute(database, &format!("INSERT INTO sys_notice(id,tenant_id,title,content,status,created_by,del_flag,created_at,updated_at) VALUES({id},'system','Scope-fixture-original','原始','1',{owner},'0','2026-08-20 00:00:00','2026-08-20 00:00:00')")).await?;
    }
    Ok(Arc::new(NoticeService::new(generated::notice::port(
        ControlDatabaseCluster::single(database.clone()),
    ))))
}

async fn restore(database: &DatabaseConnection) -> Result<(), String> {
    execute(database, "UPDATE sys_notice SET title='Scope-fixture-original',content='原始',`type`=NULL,status='1',del_flag='0',updated_at='2026-08-20 00:00:00' WHERE tenant_id='system' AND id IN(91001,91002,91003,91004)").await
}

async fn image<C: ConnectionTrait>(database: &C, id: i64) -> Result<String, String> {
    database.query_one_raw(Statement::from_sql_and_values(DbBackend::MySql,
        "SELECT CAST(JSON_OBJECT('id',id,'tenant_id',tenant_id,'title',title,'content',content,'type',`type`,'status',status,'created_by',created_by,'del_flag',del_flag,'created_at',created_at,'updated_at',updated_at) AS CHAR) AS item FROM sys_notice WHERE id=?", [id.into()]))
        .await.map_err(db_error)?.ok_or("缺少 Notice 前后像")?.try_get("","item").map_err(|error| error.to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notice_scopes_use_real_sql_for_list_detail_update_and_delete() {
    run_mysql_test("notice-scope", |database| async move {
        let service = prepare(&database).await?;
        for (scope, expected) in [
            (DataScope::All, vec![91001, 91002, 91003, 91004]),
            (DataScope::SelfOnly, vec![91001]),
            (DataScope::Dept, vec![91001]),
            (DataScope::DeptAndChildren, vec![91001, 91002]),
            (DataScope::Custom, vec![91001, 91003]),
        ] {
            restore(&database).await?;
            let actor = actor(scope);
            let page = service
                .find_by_page(&actor, list())
                .await
                .map_err(|error| error.to_string())?;
            let mut ids = page.records.iter().map(|item| item.id).collect::<Vec<_>>();
            ids.sort_unstable();
            assert_eq!(ids, expected);
            assert_eq!(page.total, expected.len() as u64);
            assert!(
                service
                    .find_by_id(&actor, 91999)
                    .await
                    .map_err(|error| error.to_string())?
                    .is_none()
            );
            for id in 91001..=91004 {
                let allowed = expected.contains(&id);
                assert_eq!(
                    service
                        .find_by_id(&actor, id)
                        .await
                        .map_err(|error| error.to_string())?
                        .is_some(),
                    allowed
                );
                verify_write(&database, &service, &actor, id, allowed, false).await?;
                verify_write(&database, &service, &actor, id, allowed, true).await?;
            }
        }
        Ok(())
    })
    .await;
}

async fn verify_write(
    database: &DatabaseConnection,
    service: &NoticeService,
    actor: &ActorContext,
    id: i64,
    allowed: bool,
    delete: bool,
) -> Result<(), String> {
    restore(database).await?;
    let before = image(database, id).await?;
    let result = if delete {
        service.delete(actor, id).await
    } else {
        service.update(actor, id, command()).await.map(|_| ())
    };
    if allowed {
        result.map_err(|error| error.to_string())?;
        assert_ne!(image(database, id).await?, before);
    } else {
        assert!(matches!(result, Err(AppError::NotFound(_))));
        assert_eq!(image(database, id).await?, before, "拒绝写入后完整记录不变");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notice_lock_rechecks_owner_after_a_concurrent_owner_change() {
    run_mysql_test("notice-owner-race", |database| async move {
        let service = prepare(&database).await?;
        let blocking = database.begin().await.map_err(db_error)?;
        blocking
            .query_one_raw(Statement::from_string(
                DbBackend::MySql,
                "SELECT id FROM sys_notice WHERE id=91001 FOR UPDATE",
            ))
            .await
            .map_err(db_error)?;
        let task = tokio::spawn(async move {
            service
                .update(&actor(DataScope::SelfOnly), 91001, command())
                .await
        });
        if let Err(error) = wait_for_notice_lock(&database).await {
            blocking.rollback().await.map_err(db_error)?;
            let _ = task.await;
            return Err(error);
        }
        let changed = async {
            execute(
                &blocking,
                "UPDATE sys_notice SET created_by=90022 WHERE id=91001",
            )
            .await?;
            image(&blocking, 91001).await
        }
        .await;
        let expected = match changed {
            Ok(expected) => expected,
            Err(error) => {
                blocking.rollback().await.map_err(db_error)?;
                let _ = task.await;
                return Err(error);
            }
        };
        blocking.commit().await.map_err(db_error)?;
        let result = task.await.map_err(|error| error.to_string())?;
        assert!(
            matches!(result, Err(AppError::NotFound(_))),
            "锁等待结束后不得沿用旧 owner 判断"
        );
        assert_eq!(image(&database, 91001).await?, expected);
        Ok(())
    })
    .await;
}

async fn wait_for_notice_lock(database: &DatabaseConnection) -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let row = database.query_one_raw(Statement::from_string(DbBackend::MySql,
                "SELECT COUNT(*) AS value FROM performance_schema.data_lock_waits w JOIN performance_schema.data_locks l ON l.ENGINE_LOCK_ID=w.REQUESTING_ENGINE_LOCK_ID WHERE l.OBJECT_SCHEMA=DATABASE() AND l.OBJECT_NAME='sys_notice'"))
                .await.map_err(db_error)?.ok_or("锁等待查询未返回结果")?;
            if row.try_get::<i64>("","value").map_err(|error| error.to_string())? > 0 { return Ok(()); }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.map_err(|_| "等待当前隔离库 Notice 行锁超时；不能把未发生竞争的运行计为通过".to_string())?
}

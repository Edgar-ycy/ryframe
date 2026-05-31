//! ryframe-db Repository 层集成测试
//!
//! 使用 SQLite 内存数据库测试 Permission/Post/Config/Dict/Notice/Job/Log Repo 的 CRUD 行为。

use chrono::Utc;
use ryframe_common::utils::snowflake;
use ryframe_core::repository::{PageQuery, Repository};
use ryframe_db::{
    ConfigRepository, DictDataRepository, DictTypeRepository, JobRepository, LoginInfoRepository,
    NoticeRepository, OperLogRepository, PermissionRepository, PostRepository, RoleRepository,
    entities::{
        config, dict_data, dict_type, job, login_info, notice, oper_log, permission, post, role,
    },
};
use sea_orm::{Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

fn now() -> chrono::DateTime<Utc> {
    Utc::now()
}

async fn setup_test_db() -> DatabaseConnection {
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("连接 SQLite 内存数据库失败");
    ryframe_db::migration::Migrator::up(&db, None)
        .await
        .expect("数据库迁移失败");
    db
}

// ==================== PermissionRepository ====================

#[tokio::test]
async fn test_permission_repo_crud() {
    let db = setup_test_db().await;
    let repo = PermissionRepository;

    let perm = permission::Model {
        id: snowflake::next_snowflake_id(),
        name: "用户列表".into(),
        code: "system:user:list".into(),
        parent_id: None,
        perm_type: "api".into(),
        path: Some("/api/v1/system/users".into()),
        icon: None,
        sort: 1,
        status: "1".into(),
        created_at: now(),
        updated_at: now(),
    };
    let inserted = repo.insert(&db, perm).await.unwrap();
    assert_eq!(inserted.code, "system:user:list");

    let found = repo.find_by_id(&db, inserted.id).await.unwrap().unwrap();
    assert_eq!(found.name, "用户列表");

    repo.delete(&db, inserted.id).await.unwrap();
    assert!(repo.find_by_id(&db, inserted.id).await.unwrap().is_none());
}

#[tokio::test]
async fn test_permission_repo_assign_and_find_role_perms() {
    let db = setup_test_db().await;
    let perm_repo = PermissionRepository;
    let role_repo = RoleRepository;

    let r = role_repo
        .insert(
            &db,
            role::Model {
                id: snowflake::next_snowflake_id(),
                name: "测试角色".into(),
                code: "test_role".into(),
                data_scope: role::Model::DATA_SCOPE_ALL.to_string(),
                status: "1".into(),
                sort: 0,
                remark: None,
                del_flag: role::Model::DEL_FLAG_NORMAL.to_string(),
                created_at: now(),
                updated_at: now(),
            },
        )
        .await
        .unwrap();

    let p1 = perm_repo
        .insert(
            &db,
            permission::Model {
                id: snowflake::next_snowflake_id(),
                name: "查询".into(),
                code: "system:user:query".into(),
                parent_id: None,
                perm_type: "api".into(),
                path: None,
                icon: None,
                sort: 1,
                status: "1".into(),
                created_at: now(),
                updated_at: now(),
            },
        )
        .await
        .unwrap();
    let p2 = perm_repo
        .insert(
            &db,
            permission::Model {
                id: snowflake::next_snowflake_id(),
                name: "删除".into(),
                code: "system:user:delete".into(),
                parent_id: None,
                perm_type: "api".into(),
                path: None,
                icon: None,
                sort: 2,
                status: "1".into(),
                created_at: now(),
                updated_at: now(),
            },
        )
        .await
        .unwrap();

    perm_repo
        .assign_perms(&db, r.id, &[p1.id, p2.id])
        .await
        .unwrap();
    let perms = perm_repo.find_role_perms(&db, &[r.id]).await.unwrap();
    assert_eq!(perms.len(), 2);

    // 重新分配
    perm_repo.assign_perms(&db, r.id, &[p1.id]).await.unwrap();
    let perms = perm_repo.find_role_perms(&db, &[r.id]).await.unwrap();
    assert_eq!(perms.len(), 1);
}

#[tokio::test]
async fn test_permission_repo_find_all() {
    let db = setup_test_db().await;
    let repo = PermissionRepository;

    for i in 0..3 {
        repo.insert(
            &db,
            permission::Model {
                id: snowflake::next_snowflake_id(),
                name: format!("perm_{}", i),
                code: format!("code:{}", i),
                parent_id: None,
                perm_type: "api".into(),
                path: None,
                icon: None,
                sort: i,
                status: "1".into(),
                created_at: now(),
                updated_at: now(),
            },
        )
        .await
        .unwrap();
    }
    let all = repo.find_all(&db).await.unwrap();
    assert_eq!(all.len(), 3);
}

// ==================== PostRepository ====================

#[tokio::test]
async fn test_post_repo_crud() {
    let db = setup_test_db().await;
    let repo = PostRepository;

    let p = post::Model {
        id: snowflake::next_snowflake_id(),
        name: "研发经理".into(),
        code: "dev_mgr".into(),
        sort: 1,
        status: post::Model::STATUS_NORMAL.to_string(),
        remark: None,
        del_flag: post::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now(),
        updated_at: now(),
    };
    let inserted = repo.insert(&db, p).await.unwrap();
    assert_eq!(inserted.code, "dev_mgr");

    let found = repo.find_by_id(&db, inserted.id).await.unwrap().unwrap();
    assert_eq!(found.name, "研发经理");

    repo.delete(&db, inserted.id).await.unwrap();
    assert!(repo.find_by_id(&db, inserted.id).await.unwrap().is_none());
}

#[tokio::test]
async fn test_post_repo_find_by_code() {
    let db = setup_test_db().await;
    let repo = PostRepository;

    for code in ["pm", "qa"] {
        repo.insert(
            &db,
            post::Model {
                id: snowflake::next_snowflake_id(),
                name: code.into(),
                code: code.into(),
                sort: 0,
                status: post::Model::STATUS_NORMAL.to_string(),
                remark: None,
                del_flag: post::Model::DEL_FLAG_NORMAL.to_string(),
                created_at: now(),
                updated_at: now(),
            },
        )
        .await
        .unwrap();
    }
    assert!(repo.find_by_code(&db, "pm").await.unwrap().is_some());
    assert!(repo.find_by_code(&db, "unknown").await.unwrap().is_none());
}

#[tokio::test]
async fn test_post_repo_filtered_pagination() {
    let db = setup_test_db().await;
    let repo = PostRepository;

    for (name, code, status) in [
        ("经理", "mgr", post::Model::STATUS_NORMAL),
        ("实习生", "intern", post::Model::STATUS_DISABLED),
    ] {
        repo.insert(
            &db,
            post::Model {
                id: snowflake::next_snowflake_id(),
                name: name.into(),
                code: code.into(),
                sort: 0,
                status: status.to_string(),
                remark: None,
                del_flag: post::Model::DEL_FLAG_NORMAL.to_string(),
                created_at: now(),
                updated_at: now(),
            },
        )
        .await
        .unwrap();
    }

    let page = repo
        .find_by_page_filtered(&db, PageQuery::default(), Some("经理"), None, None)
        .await
        .unwrap();
    assert_eq!(page.records.len(), 1);
}

// ==================== ConfigRepository ====================

#[tokio::test]
async fn test_config_repo_find_by_key() {
    let db = setup_test_db().await;
    let repo = ConfigRepository;

    repo.insert(
        &db,
        config::Model {
            id: snowflake::next_snowflake_id(),
            name: "初始密码".into(),
            key: "sys.user.initPassword".into(),
            value: "123456".into(),
            remark: None,
            del_flag: config::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: now(),
            updated_at: now(),
        },
    )
    .await
    .unwrap();

    let found = repo
        .find_by_key(&db, "sys.user.initPassword")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.value, "123456");
    assert!(repo.find_by_key(&db, "nope").await.unwrap().is_none());
}

#[tokio::test]
async fn test_config_repo_pagination_boundary() {
    let db = setup_test_db().await;
    let repo = ConfigRepository;

    for i in 0..25 {
        repo.insert(
            &db,
            config::Model {
                id: snowflake::next_snowflake_id(),
                name: format!("配置{}", i),
                key: format!("config_{:02}", i),
                value: format!("v{}", i),
                remark: None,
                del_flag: config::Model::DEL_FLAG_NORMAL.to_string(),
                created_at: now(),
                updated_at: now(),
            },
        )
        .await
        .unwrap();
    }

    let p1 = repo
        .find_by_page(
            &db,
            PageQuery {
                page: 1,
                page_size: 10,
            },
        )
        .await
        .unwrap();
    assert_eq!(p1.records.len(), 10);
    assert_eq!(p1.total, 25);

    let p3 = repo
        .find_by_page(
            &db,
            PageQuery {
                page: 3,
                page_size: 10,
            },
        )
        .await
        .unwrap();
    assert_eq!(p3.records.len(), 5);
}

// ==================== DictRepository ====================

#[tokio::test]
async fn test_dict_type_repo_crud() {
    let db = setup_test_db().await;
    let repo = DictTypeRepository;

    let dt = dict_type::Model {
        id: snowflake::next_snowflake_id(),
        name: "用户性别".into(),
        code: "sys_user_sex".into(),
        status: dict_type::Model::STATUS_NORMAL.to_string(),
        remark: None,
        del_flag: dict_type::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now(),
        updated_at: now(),
    };
    let inserted = repo.insert(&db, dt).await.unwrap();
    assert_eq!(inserted.code, "sys_user_sex");

    repo.delete(&db, inserted.id).await.unwrap();
    assert!(repo.find_by_id(&db, inserted.id).await.unwrap().is_none());
}

#[tokio::test]
async fn test_dict_data_repo_find_by_type_code() {
    let db = setup_test_db().await;
    let type_repo = DictTypeRepository;
    let data_repo = DictDataRepository;

    let dt = type_repo
        .insert(
            &db,
            dict_type::Model {
                id: snowflake::next_snowflake_id(),
                name: "通用状态".into(),
                code: "sys_normal_disable".into(),
                status: dict_type::Model::STATUS_NORMAL.to_string(),
                remark: None,
                del_flag: dict_type::Model::DEL_FLAG_NORMAL.to_string(),
                created_at: now(),
                updated_at: now(),
            },
        )
        .await
        .unwrap();

    for (label, value, sort) in [("正常", "0", 1), ("停用", "1", 2)] {
        data_repo
            .insert(
                &db,
                dict_data::Model {
                    id: snowflake::next_snowflake_id(),
                    type_code: dt.code.clone(),
                    label: label.into(),
                    value: value.into(),
                    sort,
                    status: dict_data::Model::STATUS_NORMAL.to_string(),
                    css_class: None,
                    remark: None,
                    del_flag: dict_data::Model::DEL_FLAG_NORMAL.to_string(),
                    created_at: now(),
                    updated_at: now(),
                },
            )
            .await
            .unwrap();
    }

    let data = data_repo
        .find_by_type_code(&db, "sys_normal_disable")
        .await
        .unwrap();
    assert_eq!(data.len(), 2);
    assert_eq!(data[0].label, "正常");
}

// ==================== NoticeRepository ====================

#[tokio::test]
async fn test_notice_repo_crud() {
    let db = setup_test_db().await;
    let repo = NoticeRepository;

    let n = notice::Model {
        id: snowflake::next_snowflake_id(),
        title: "系统维护通知".into(),
        content: "系统将于凌晨维护".into(),
        r#type: Some("1".into()),
        status: notice::Model::STATUS_PUBLISHED.to_string(),
        created_by: Some(1),
        del_flag: notice::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now(),
        updated_at: now(),
    };
    let inserted = repo.insert(&db, n).await.unwrap();
    assert_eq!(inserted.title, "系统维护通知");

    let found = repo.find_by_id(&db, inserted.id).await.unwrap().unwrap();
    assert_eq!(found.r#type.as_deref(), Some("1"));

    repo.delete(&db, inserted.id).await.unwrap();
    assert!(repo.find_by_id(&db, inserted.id).await.unwrap().is_none());
}

#[tokio::test]
async fn test_notice_repo_filtered_pagination() {
    let db = setup_test_db().await;
    let repo = NoticeRepository;

    for (title, ntype) in [
        ("通知A", Some("1".into())),
        ("通知B", Some("1".into())),
        ("公告C", Some("2".into())),
    ] {
        repo.insert(
            &db,
            notice::Model {
                id: snowflake::next_snowflake_id(),
                title: title.into(),
                content: "内容".into(),
                r#type: ntype,
                status: notice::Model::STATUS_PUBLISHED.to_string(),
                created_by: None,
                del_flag: notice::Model::DEL_FLAG_NORMAL.to_string(),
                created_at: now(),
                updated_at: now(),
            },
        )
        .await
        .unwrap();
    }

    // 按类型过滤
    let page = repo
        .find_by_page_filtered(&db, PageQuery::default(), None, Some("2"), None)
        .await
        .unwrap();
    assert_eq!(page.records.len(), 1);

    // 按标题模糊搜索
    let page = repo
        .find_by_page_filtered(&db, PageQuery::default(), Some("通知"), None, None)
        .await
        .unwrap();
    assert_eq!(page.records.len(), 2);
}

// ==================== JobRepository ====================

#[tokio::test]
async fn test_job_repo_crud() {
    let db = setup_test_db().await;
    let repo = JobRepository;

    let j = job::Model {
        id: snowflake::next_snowflake_id(),
        name: "日志清理".into(),
        group_name: "DEFAULT".into(),
        cron_expr: "0 0 2 * * ?".into(),
        misfire_policy: "1".into(),
        concurrent: "1".into(),
        status: job::Model::STATUS_NORMAL.to_string(),
        remark: None,
        create_time: now(),
        update_time: now(),
    };
    let inserted = repo.insert(&db, j).await.unwrap();
    assert_eq!(inserted.cron_expr, "0 0 2 * * ?");

    repo.delete(&db, inserted.id).await.unwrap();
    assert!(repo.find_by_id(&db, inserted.id).await.unwrap().is_none());
}

#[tokio::test]
async fn test_job_repo_update_status() {
    let db = setup_test_db().await;
    let repo = JobRepository;

    let j = repo
        .insert(
            &db,
            job::Model {
                id: snowflake::next_snowflake_id(),
                name: "测试任务".into(),
                group_name: "DEFAULT".into(),
                cron_expr: "0/10 * * * * ?".into(),
                misfire_policy: "1".into(),
                concurrent: "1".into(),
                status: job::Model::STATUS_NORMAL.to_string(),
                remark: None,
                create_time: now(),
                update_time: now(),
            },
        )
        .await
        .unwrap();

    repo.update_status(&db, j.id, job::Model::STATUS_PAUSED.to_string())
        .await
        .unwrap();

    let found = repo.find_by_id(&db, j.id).await.unwrap().unwrap();
    assert_eq!(found.status, job::Model::STATUS_PAUSED);
}

#[tokio::test]
async fn test_job_repo_find_all_enabled() {
    let db = setup_test_db().await;
    let repo = JobRepository;

    for (name, status) in [
        ("任务A", job::Model::STATUS_NORMAL),
        ("任务B", job::Model::STATUS_PAUSED),
        ("任务C", job::Model::STATUS_NORMAL),
    ] {
        repo.insert(
            &db,
            job::Model {
                id: snowflake::next_snowflake_id(),
                name: name.into(),
                group_name: "DEFAULT".into(),
                cron_expr: "0/30 * * * * ?".into(),
                misfire_policy: "1".into(),
                concurrent: "1".into(),
                status: status.to_string(),
                remark: None,
                create_time: now(),
                update_time: now(),
            },
        )
        .await
        .unwrap();
    }
    let enabled = repo.find_all_enabled(&db).await.unwrap();
    assert_eq!(enabled.len(), 2);
}

#[tokio::test]
async fn test_job_repo_update_cron() {
    let db = setup_test_db().await;
    let repo = JobRepository;

    let j = repo
        .insert(
            &db,
            job::Model {
                id: snowflake::next_snowflake_id(),
                name: "动态任务".into(),
                group_name: "DEFAULT".into(),
                cron_expr: "0 0 1 * * ?".into(),
                misfire_policy: "1".into(),
                concurrent: "1".into(),
                status: job::Model::STATUS_NORMAL.to_string(),
                remark: None,
                create_time: now(),
                update_time: now(),
            },
        )
        .await
        .unwrap();

    repo.update_cron(
        &db,
        j.id,
        Some("0 0 3 * * ?".into()),
        Some(job::Model::STATUS_PAUSED.to_string()),
        Some("已暂停".into()),
    )
    .await
    .unwrap();

    let found = repo.find_by_id(&db, j.id).await.unwrap().unwrap();
    assert_eq!(found.cron_expr, "0 0 3 * * ?");
    assert_eq!(found.status, job::Model::STATUS_PAUSED);
    assert_eq!(found.remark.as_deref(), Some("已暂停"));
}

// ==================== LoginInfoRepository ====================

#[tokio::test]
async fn test_login_info_repo_insert_and_query() {
    let db = setup_test_db().await;
    let repo = LoginInfoRepository;

    for (user, status) in [
        ("admin", login_info::Model::STATUS_SUCCESS),
        ("user1", login_info::Model::STATUS_SUCCESS),
        ("user2", login_info::Model::STATUS_FAIL),
    ] {
        repo.insert(
            &db,
            login_info::Model {
                id: snowflake::next_snowflake_id(),
                user_name: user.into(),
                ipaddr: "127.0.0.1".into(),
                login_location: Some("本地".into()),
                browser: Some("Chrome".into()),
                os: Some("Windows".into()),
                status: status.to_string(),
                msg: Some("登录成功".into()),
                login_time: Utc::now(),
            },
        )
        .await
        .unwrap();
    }

    // 分页查询
    let page = repo.find_by_page(&db, PageQuery::default()).await.unwrap();
    assert_eq!(page.total, 3);

    // 按用户名过滤
    let page = repo
        .find_by_page_filtered(&db, PageQuery::default(), Some("user1"), None, None, None)
        .await
        .unwrap();
    assert_eq!(page.records.len(), 1);

    // 按状态过滤
    let page = repo
        .find_by_page_filtered(
            &db,
            PageQuery::default(),
            None,
            Some(login_info::Model::STATUS_FAIL.to_string()),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(page.records.len(), 1);
}

#[tokio::test]
async fn test_login_info_repo_clean_all() {
    let db = setup_test_db().await;
    let repo = LoginInfoRepository;

    for i in 0..3 {
        repo.insert(
            &db,
            login_info::Model {
                id: snowflake::next_snowflake_id(),
                user_name: format!("user_{}", i),
                ipaddr: "127.0.0.1".into(),
                login_location: None,
                browser: None,
                os: None,
                status: login_info::Model::STATUS_SUCCESS.to_string(),
                msg: None,
                login_time: Utc::now(),
            },
        )
        .await
        .unwrap();
    }

    let deleted = repo.clean_all(&db).await.unwrap();
    assert_eq!(deleted, 3);

    let page = repo.find_by_page(&db, PageQuery::default()).await.unwrap();
    assert_eq!(page.total, 0);
}

// ==================== OperLogRepository ====================

#[tokio::test]
async fn test_oper_log_repo_insert_and_query() {
    let db = setup_test_db().await;
    let repo = OperLogRepository;

    for (oper_name, status) in [
        ("admin", oper_log::Model::STATUS_SUCCESS),
        ("admin", oper_log::Model::STATUS_SUCCESS),
        ("user1", oper_log::Model::STATUS_FAIL),
    ] {
        repo.insert(
            &db,
            oper_log::Model {
                id: snowflake::next_snowflake_id(),
                title: format!("{}操作", oper_name),
                business_type: "INSERT".into(),
                method: "UserServiceImpl.create".into(),
                request_method: "POST".into(),
                oper_name: oper_name.into(),
                oper_url: "/api/v1/system/users".into(),
                oper_ip: "127.0.0.1".into(),
                oper_location: Some("本地".into()),
                oper_param: Some("{}".into()),
                json_result: Some("{}".into()),
                status: status.to_string(),
                error_msg: None,
                oper_time: Utc::now(),
                cost_time: 23,
            },
        )
        .await
        .unwrap();
    }

    let page = repo.find_by_page(&db, PageQuery::default()).await.unwrap();
    assert_eq!(page.total, 3);

    // 按操作人员过滤
    let page = repo
        .find_by_page_filtered(&db, PageQuery::default(), Some("user1"), None, None, None)
        .await
        .unwrap();
    assert_eq!(page.records.len(), 1);

    // 按状态过滤
    let page = repo
        .find_by_page_filtered(
            &db,
            PageQuery::default(),
            None,
            Some(oper_log::Model::STATUS_FAIL.to_string()),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(page.records.len(), 1);
}

#[tokio::test]
async fn test_oper_log_repo_clean_all() {
    let db = setup_test_db().await;
    let repo = OperLogRepository;

    for i in 0..3 {
        repo.insert(
            &db,
            oper_log::Model {
                id: snowflake::next_snowflake_id(),
                title: format!("操作{}", i),
                business_type: "QUERY".into(),
                method: "UserServiceImpl.find".into(),
                request_method: "GET".into(),
                oper_name: "admin".into(),
                oper_url: "/api/v1/system".into(),
                oper_ip: "127.0.0.1".into(),
                oper_location: None,
                oper_param: None,
                json_result: None,
                status: oper_log::Model::STATUS_SUCCESS.to_string(),
                error_msg: None,
                oper_time: Utc::now(),
                cost_time: 10,
            },
        )
        .await
        .unwrap();
    }

    let deleted = repo.clean_all(&db).await.unwrap();
    assert_eq!(deleted, 3);
}

//! ryframe-db Repository 层集成测试
//!
//! 使用 SQLite 内存数据库测试 Permission/Post/Config/Dict/Notice/Log Repo 的 CRUD 行为。

mod common;
use common::setup_test_db;

use chrono::Utc;
use ryframe_core::auto_fill::{AutoFill, FillContext};
use ryframe_core::repository::{PageQuery, Repository};
use ryframe_db::{
    ConfigRepository, DictDataRepository, DictTypeRepository, LoginInfoRepository,
    NoticeRepository, OperLogRepository, PermissionRepository, PostRepository, RoleRepository,
    entities::{
        config, dict_data, dict_type, login_info, notice, oper_log, permission, post, role,
    },
};

fn now() -> chrono::DateTime<Utc> {
    Utc::now()
}

// ==================== 辅助构造器（使用 AutoFill 自动生成雪花 ID） ====================

fn make_permission(name: &str, code: &str, sort: i32) -> permission::Model {
    let mut m = permission::Model {
        id: 0,
        name: name.into(),
        code: code.into(),
        parent_id: None,
        perm_type: "api".into(),
        path: None,
        http_method: None,
        icon: None,
        sort,
        status: "1".into(),
        created_at: now(),
        updated_at: now(),
    };
    m.fill_on_insert(&FillContext::new());
    m
}

fn make_role(name: &str, code: &str) -> role::Model {
    let mut m = role::Model {
        id: 0,
        name: name.into(),
        code: code.into(),
        data_scope: role::Model::DATA_SCOPE_ALL.to_string(),
        status: "1".into(),
        sort: 0,
        remark: None,
        del_flag: role::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now(),
        updated_at: now(),
    };
    m.fill_on_insert(&FillContext::new());
    m
}

fn make_post(name: &str, code: &str, status: &str) -> post::Model {
    let mut m = post::Model {
        id: 0,
        name: name.into(),
        code: code.into(),
        sort: 0,
        status: status.into(),
        remark: None,
        del_flag: post::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now(),
        updated_at: now(),
    };
    m.fill_on_insert(&FillContext::new());
    m
}

fn make_config(key: &str, value: &str) -> config::Model {
    let mut m = config::Model {
        id: 0,
        name: key.into(),
        key: key.into(),
        value: value.into(),
        remark: None,
        del_flag: config::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now(),
        updated_at: now(),
    };
    m.fill_on_insert(&FillContext::new());
    m
}

fn make_dict_type(name: &str, code: &str) -> dict_type::Model {
    let mut m = dict_type::Model {
        id: 0,
        name: name.into(),
        code: code.into(),
        status: dict_type::Model::STATUS_NORMAL.to_string(),
        remark: None,
        del_flag: dict_type::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now(),
        updated_at: now(),
    };
    m.fill_on_insert(&FillContext::new());
    m
}

fn make_dict_data(type_code: &str, label: &str, value: &str, sort: i32) -> dict_data::Model {
    let mut m = dict_data::Model {
        id: 0,
        type_code: type_code.into(),
        label: label.into(),
        value: value.into(),
        sort,
        status: dict_data::Model::STATUS_NORMAL.to_string(),
        css_class: None,
        remark: None,
        del_flag: dict_data::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now(),
        updated_at: now(),
    };
    m.fill_on_insert(&FillContext::new());
    m
}

fn make_notice(title: &str, ntype: Option<&str>) -> notice::Model {
    let mut m = notice::Model {
        id: 0,
        title: title.into(),
        content: "内容".into(),
        r#type: ntype.map(|s| s.to_string()),
        status: notice::Model::STATUS_PUBLISHED.to_string(),
        created_by: None,
        del_flag: notice::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now(),
        updated_at: now(),
    };
    m.fill_on_insert(&FillContext::new());
    m
}

fn make_login_info(user: &str, status: &str) -> login_info::Model {
    let mut m = login_info::Model {
        id: 0,
        user_name: user.into(),
        ipaddr: "127.0.0.1".into(),
        login_location: Some("本地".into()),
        browser: Some("Chrome".into()),
        os: Some("Windows".into()),
        status: status.into(),
        msg: Some("登录成功".into()),
        login_time: Utc::now(),
    };
    m.fill_on_insert(&FillContext::new());
    m
}

fn make_oper_log(oper_name: &str, status: &str) -> oper_log::Model {
    let mut m = oper_log::Model {
        id: 0,
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
        status: status.into(),
        error_msg: None,
        oper_time: Utc::now(),
        cost_time: 23,
    };
    m.fill_on_insert(&FillContext::new());
    m
}

// ==================== PermissionRepository ====================

#[tokio::test]
async fn test_permission_repo_crud() {
    let db = setup_test_db().await;
    let repo = PermissionRepository;

    let perm = make_permission("用户列表", "system:user:list", 1);
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
        .insert(&db, make_role("测试角色", "test_role"))
        .await
        .unwrap();
    let p1 = perm_repo
        .insert(&db, make_permission("查询", "system:user:query", 1))
        .await
        .unwrap();
    let p2 = perm_repo
        .insert(&db, make_permission("删除", "system:user:delete", 2))
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
            make_permission(&format!("perm_{}", i), &format!("code:{}", i), i),
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

    let p = make_post("研发经理", "dev_mgr", post::Model::STATUS_NORMAL);
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
        repo.insert(&db, make_post(code, code, post::Model::STATUS_NORMAL))
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

    repo.insert(&db, make_post("经理", "mgr", post::Model::STATUS_NORMAL))
        .await
        .unwrap();
    repo.insert(
        &db,
        make_post("实习生", "intern", post::Model::STATUS_DISABLED),
    )
    .await
    .unwrap();

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

    repo.insert(&db, make_config("sys.user.initPassword", "123456"))
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
            make_config(&format!("config_{:02}", i), &format!("v{}", i)),
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

    let dt = make_dict_type("用户性别", "sys_user_sex");
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
        .insert(&db, make_dict_type("通用状态", "sys_normal_disable"))
        .await
        .unwrap();

    for (label, value, sort) in [("正常", "0", 1), ("停用", "1", 2)] {
        data_repo
            .insert(&db, make_dict_data(&dt.code, label, value, sort))
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

    let n = make_notice("系统维护通知", Some("1"));
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

    repo.insert(&db, make_notice("通知A", Some("1")))
        .await
        .unwrap();
    repo.insert(&db, make_notice("通知B", Some("1")))
        .await
        .unwrap();
    repo.insert(&db, make_notice("公告C", Some("2")))
        .await
        .unwrap();

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

// ==================== LoginInfoRepository ====================

#[tokio::test]
async fn test_login_info_repo_insert_and_query() {
    let db = setup_test_db().await;
    let repo = LoginInfoRepository;

    repo.insert(
        &db,
        make_login_info("admin", login_info::Model::STATUS_SUCCESS),
    )
    .await
    .unwrap();
    repo.insert(
        &db,
        make_login_info("user1", login_info::Model::STATUS_SUCCESS),
    )
    .await
    .unwrap();
    repo.insert(
        &db,
        make_login_info("user2", login_info::Model::STATUS_FAIL),
    )
    .await
    .unwrap();

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
        let mut m = login_info::Model {
            id: 0,
            user_name: format!("user_{}", i),
            ipaddr: "127.0.0.1".into(),
            login_location: None,
            browser: None,
            os: None,
            status: login_info::Model::STATUS_SUCCESS.to_string(),
            msg: None,
            login_time: Utc::now(),
        };
        m.fill_on_insert(&FillContext::new());
        repo.insert(&db, m).await.unwrap();
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

    repo.insert(&db, make_oper_log("admin", oper_log::Model::STATUS_SUCCESS))
        .await
        .unwrap();
    repo.insert(&db, make_oper_log("admin", oper_log::Model::STATUS_SUCCESS))
        .await
        .unwrap();
    repo.insert(&db, make_oper_log("user1", oper_log::Model::STATUS_FAIL))
        .await
        .unwrap();

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
        let mut m = oper_log::Model {
            id: 0,
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
        };
        m.fill_on_insert(&FillContext::new());
        repo.insert(&db, m).await.unwrap();
    }

    let deleted = repo.clean_all(&db).await.unwrap();
    assert_eq!(deleted, 3);
}

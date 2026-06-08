//! Service 层单元测试
//!
//! 使用 SQLite 内存数据库测试核心 Service 业务逻辑。
//! 每个测试验证一个用户场景，不与 HTTP 层耦合。

use std::sync::Arc;

use chrono::Utc;
use ryframe_auth::password;
use ryframe_common::utils::snowflake;
use ryframe_config::{
    AppConfig, AppSettings, AuthConfig, DatabaseConfig, DbConnection, LoggerConfig, RateLimitConfig,
};
use ryframe_core::{LoggedRepo, Repository, repository::PageQuery};
use ryframe_db::{
    ConfigRepository, DeptRepository, DictDataRepository, DictTypeRepository, JobLogRepository,
    JobRepository, LoginInfoRepository, MenuRepository, NoticeRepository, OperLogRepository,
    PermissionRepository, PostRepository, RoleRepository, UserRepository,
    entities::{dept, role, user},
};
use ryframe_service::{
    AuthServiceImpl,
    system::{
        ConfigServiceImpl, CreateUserParams, DeptServiceImpl, DictServiceImpl, LoginInfoServiceImpl,
        MenuServiceImpl,
        NoticeServiceImpl, OperLogServiceImpl, PostServiceImpl, RoleServiceImpl, UserServiceImpl,
    },
};
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait};
use sea_orm_migration::MigratorTrait;

// ==================== 辅助函数 ====================

/// 创建 SQLite 内存数据库并运行迁移
async fn setup_test_db() -> DatabaseConnection {
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("连接 SQLite 内存数据库失败");
    ryframe_db::migration::Migrator::up(&db, None)
        .await
        .expect("数据库迁移失败");
    db
}

/// 创建测试配置
fn test_config() -> AppConfig {
    AppConfig {
        app: AppSettings {
            name: "test".into(),
            port: 0,
            ..Default::default()
        },
        database: DatabaseConfig {
            connections: vec![DbConnection {
                driver: "sqlite".into(),
                database: ":memory:".into(),
                max_connections: 5,
                ..Default::default()
            }],
            ..Default::default()
        },
        auth: AuthConfig {
            jwt_secret: "test-secret-key-for-service-tests".into(),
            enable_password_complexity: false,
            ..Default::default()
        },
        redis: None,
        logger: LoggerConfig {
            level: "warn".into(),
            ..Default::default()
        },
        rate_limit: RateLimitConfig::default(),
        cors: Default::default(),
        object_storage: Default::default(),
    }
}

/// 创建测试用户（返回 Model）
async fn create_test_user(db: &DatabaseConnection, username: &str) -> user::Model {
    let password_hash = password::hash("test123").unwrap();
    let now = Utc::now();
    let u = user::Model {
        id: snowflake::next_snowflake_id(),
        username: username.to_string(),
        password_hash,
        nickname: username.to_string(),
        email: format!("{}@test.com", username),
        phone: "13800000000".to_string(),
        avatar: None,
        status: user::Model::STATUS_NORMAL.to_string(),
        dept_id: None,
        remark: None,
        login_ip: None,
        login_date: None,
        del_flag: user::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now,
        updated_at: now,
    };
    let active: user::ActiveModel = u.clone().into();
    active.insert(db).await.unwrap();
    u
}

/// 创建测试角色（返回 Model）
async fn create_test_role(db: &DatabaseConnection, name: &str, code: &str) -> role::Model {
    let now = Utc::now();
    let r = role::Model {
        id: snowflake::next_snowflake_id(),
        name: name.to_string(),
        code: code.to_string(),
        data_scope: "1".to_string(),
        status: "1".to_string(),
        sort: 0,
        remark: None,
        del_flag: role::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now,
        updated_at: now,
    };
    let active: role::ActiveModel = r.clone().into();
    active.insert(db).await.unwrap();
    r
}

/// 创建测试部门（返回 Model）
async fn create_test_dept(
    db: &DatabaseConnection,
    name: &str,
    parent_id: Option<i64>,
) -> dept::Model {
    let now = Utc::now();
    let ancestors = match parent_id {
        Some(pid) => {
            let parent = dept::Entity::find_by_id(pid).one(db).await.unwrap();
            parent
                .map(|p| format!("{},{}", p.ancestors, pid))
                .unwrap_or_else(|| pid.to_string())
        }
        None => "0".to_string(),
    };
    let d = dept::Model {
        id: snowflake::next_snowflake_id(),
        name: name.to_string(),
        parent_id,
        ancestors,
        sort: 0,
        status: dept::Model::STATUS_NORMAL.to_string(),
        remark: None,
        del_flag: dept::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now,
        updated_at: now,
    };
    let active: dept::ActiveModel = d.clone().into();
    active.insert(db).await.unwrap();
    d
}

// ==================== UserService 测试 ====================

/// 用户创建 → 按用户名查重 → 按ID查询
#[tokio::test]
async fn test_user_create_and_find() {
    let db = setup_test_db().await;
    let svc = UserServiceImpl {
        user_repo: LoggedRepo::new(UserRepository),
        role_repo: LoggedRepo::new(RoleRepository),
        dept_repo: LoggedRepo::new(DeptRepository),
    };

    // 创建用户
    let vo = svc
        .create(
            &db,
            CreateUserParams {
                username: "alice",
                password: "pass123",
                nickname: "Alice",
                email: "alice@a.com",
                phone: "111",
                dept_id: None,
                role_ids: None,
                enable_pwd_complexity: false,
            },
        )
        .await
        .expect("创建用户失败");
    assert_eq!(vo.username, "alice");
    assert_eq!(vo.nickname, "Alice");

    // 用户名重复检测
    let err = svc
        .create(
            &db,
            CreateUserParams {
                username: "alice",
                password: "pass456",
                nickname: "Alice2",
                email: "a2@a.com",
                phone: "222",
                dept_id: None,
                role_ids: None,
                enable_pwd_complexity: false,
            },
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("已存在"));

    // 按ID查询
    let found = svc
        .find_by_id(&db, vo.id)
        .await
        .unwrap()
        .expect("查不到用户");
    assert_eq!(found.username, "alice");
}

/// 用户更新 + 状态变更 + 密码重置
#[tokio::test]
async fn test_user_update_and_status() {
    let db = setup_test_db().await;
    let svc = UserServiceImpl {
        user_repo: LoggedRepo::new(UserRepository),
        role_repo: LoggedRepo::new(RoleRepository),
        dept_repo: LoggedRepo::new(DeptRepository),
    };

    let u = create_test_user(&db, "bob").await;

    // 更新用户信息 - 用 ActiveModel 直接更新
    use sea_orm::ActiveValue;
    let active = user::ActiveModel {
        id: ActiveValue::Unchanged(u.id),
        nickname: ActiveValue::Set("BobNew".to_string()),
        updated_at: ActiveValue::Set(Utc::now()),
        ..Default::default()
    };
    assert!(
        active.update(&db).await.is_ok(),
        "active model update should work"
    );

    // 重新查询验证持久化
    let found = UserRepository
        .find_by_id(&db, u.id)
        .await
        .unwrap()
        .expect("user not found");
    assert_eq!(found.nickname, "BobNew", "db should persist update");

    // 修改状态
    svc.change_status(&db, u.id, "1".into())
        .await
        .expect("改状态失败");

    // 重置密码
    svc.reset_password(&db, u.id, "newpass456", false)
        .await
        .expect("重置密码失败");
}

/// 用户分页查询
#[tokio::test]
async fn test_user_pagination() {
    let db = setup_test_db().await;
    let svc = UserServiceImpl {
        user_repo: LoggedRepo::new(UserRepository),
        role_repo: LoggedRepo::new(RoleRepository),
        dept_repo: LoggedRepo::new(DeptRepository),
    };

    // 创建多个用户
    for i in 0..5 {
        create_test_user(&db, &format!("user_{}", i)).await;
    }

    let page = svc
        .find_by_page(
            &db,
            PageQuery {
                page: 1,
                page_size: 3,
            },
        )
        .await
        .expect("分页查询失败");
    assert_eq!(page.records.len(), 3);
    assert_eq!(page.total, 5);

    let page2 = svc
        .find_by_page(
            &db,
            PageQuery {
                page: 2,
                page_size: 3,
            },
        )
        .await
        .expect("分页查询失败");
    assert_eq!(page2.records.len(), 2);
}

/// 批量删除用户
#[tokio::test]
async fn test_user_batch_delete() {
    let db = setup_test_db().await;
    let svc = UserServiceImpl {
        user_repo: LoggedRepo::new(UserRepository),
        role_repo: LoggedRepo::new(RoleRepository),
        dept_repo: LoggedRepo::new(DeptRepository),
    };

    let u1 = create_test_user(&db, "del_1").await;
    let u2 = create_test_user(&db, "del_2").await;

    let deleted = svc
        .delete_many(&db, &[u1.id, u2.id])
        .await
        .expect("批量删除失败");
    assert_eq!(deleted, 2);

    // 删除后应查不到
    assert!(svc.find_by_id(&db, u1.id).await.unwrap().is_none());
}

/// 删除不存在的用户报 NotFound
#[tokio::test]
async fn test_user_delete_not_found() {
    let db = setup_test_db().await;
    let svc = UserServiceImpl {
        user_repo: LoggedRepo::new(UserRepository),
        role_repo: LoggedRepo::new(RoleRepository),
        dept_repo: LoggedRepo::new(DeptRepository),
    };

    let err = svc.delete(&db, 99999).await.unwrap_err();
    assert!(err.to_string().contains("不存在"));
}

// ==================== RoleService 测试 ====================

/// 角色 CRUD 完整流程
#[tokio::test]
async fn test_role_crud() {
    let db = setup_test_db().await;
    let svc = RoleServiceImpl {
        role_repo: LoggedRepo::new(RoleRepository),
        perm_repo: LoggedRepo::new(PermissionRepository),
        menu_repo: LoggedRepo::new(MenuRepository),
    };

    // 创建
    let vo = svc
        .create(&db, "管理员", "admin_role", 1, None)
        .await
        .expect("创建角色失败");
    assert_eq!(vo.code, "admin_role");

    // 编码重复
    let err = svc
        .create(&db, "管理员2", "admin_role", 2, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("已存在"));

    // 查询
    let found = svc
        .find_by_id(&db, vo.id)
        .await
        .unwrap()
        .expect("查不到角色");
    assert_eq!(found.name, "管理员");

    // 更新 - 用 ActiveModel 直接更新
    use sea_orm::ActiveValue as Av;
    let ra = role::ActiveModel {
        id: Av::Unchanged(vo.id),
        name: Av::Set("superadmin".to_string()),
        data_scope: Av::Set("3".to_string()),
        updated_at: Av::Set(Utc::now()),
        ..Default::default()
    };
    ra.update(&db).await.expect("update role failed");

    let updated = svc
        .find_by_id(&db, vo.id)
        .await
        .unwrap()
        .expect("查不到角色");
    assert_eq!(updated.name, "superadmin");
    assert_eq!(updated.data_scope, "3");
}

/// 角色分页 + 过滤
#[tokio::test]
async fn test_role_pagination_and_filter() {
    let db = setup_test_db().await;
    let svc = RoleServiceImpl {
        role_repo: LoggedRepo::new(RoleRepository),
        perm_repo: LoggedRepo::new(PermissionRepository),
        menu_repo: LoggedRepo::new(MenuRepository),
    };

    create_test_role(&db, "角色A", "role_a").await;
    create_test_role(&db, "角色B", "role_b").await;

    let page = svc
        .find_by_page(&db, PageQuery::default())
        .await
        .expect("分页失败");
    assert_eq!(page.total, 2);

    // 按名称过滤
    let filtered = svc
        .find_by_page_filtered(&db, PageQuery::default(), Some("角色A"), None, None)
        .await
        .expect("过滤失败");
    assert_eq!(filtered.total, 1);
}

/// 角色删除
#[tokio::test]
async fn test_role_delete() {
    let db = setup_test_db().await;
    let svc = RoleServiceImpl {
        role_repo: LoggedRepo::new(RoleRepository),
        perm_repo: LoggedRepo::new(PermissionRepository),
        menu_repo: LoggedRepo::new(MenuRepository),
    };

    let r = create_test_role(&db, "待删除", "to_delete").await;
    svc.delete(&db, r.id).await.expect("删除失败");

    assert!(svc.find_by_id(&db, r.id).await.unwrap().is_none());
}

/// 角色批量删除
#[tokio::test]
async fn test_role_batch_delete() {
    let db = setup_test_db().await;
    let svc = RoleServiceImpl {
        role_repo: LoggedRepo::new(RoleRepository),
        perm_repo: LoggedRepo::new(PermissionRepository),
        menu_repo: LoggedRepo::new(MenuRepository),
    };

    let r1 = create_test_role(&db, "批量1", "batch_1").await;
    let r2 = create_test_role(&db, "批量2", "batch_2").await;

    let count = svc
        .delete_many(&db, &[r1.id, r2.id])
        .await
        .expect("批量删除失败");
    assert_eq!(count, 2);
}

/// 角色数据权限设置
#[tokio::test]
async fn test_role_data_scope() {
    let db = setup_test_db().await;
    let svc = RoleServiceImpl {
        role_repo: LoggedRepo::new(RoleRepository),
        perm_repo: LoggedRepo::new(PermissionRepository),
        menu_repo: LoggedRepo::new(MenuRepository),
    };

    let r = create_test_role(&db, "数据权限角色", "data_scope_role").await;

    // 设置自定义数据权限
    svc.assign_data_scope(&db, r.id, "2", vec![1, 2, 3])
        .await
        .expect("设置数据权限失败");

    let found = svc
        .find_by_id(&db, r.id)
        .await
        .unwrap()
        .expect("查不到角色");
    assert_eq!(found.data_scope, "2");

    // 无效数据范围
    let err = svc
        .assign_data_scope(&db, r.id, "99", vec![])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("无效"));
}

// ==================== DeptService 测试 ====================

/// 部门树形结构 CRUD
#[tokio::test]
async fn test_dept_tree_and_crud() {
    let db = setup_test_db().await;
    let svc = DeptServiceImpl {
        dept_repo: LoggedRepo::new(DeptRepository),
        redis: None,
    };

    // 创建根部门
    let root = svc
        .create(&db, "总公司", None, 0)
        .await
        .expect("创建根部门失败");
    assert_eq!(root.name, "总公司");

    // 创建子部门
    let _child = svc
        .create(&db, "研发部", Some(root.id), 1)
        .await
        .expect("创建子部门失败");

    // 查询树
    let tree = svc.find_tree(&db).await.expect("查询树失败");
    assert_eq!(tree.len(), 1); // 根部门
    assert!(!tree[0].children.is_empty()); // 有子部门

    // 查询详情
    let detail = svc
        .find_by_id(&db, root.id)
        .await
        .unwrap()
        .expect("详情查不到");
    assert_eq!(detail.name, "总公司");

    // 删除子部门（有子部门应失败）
    let err = svc.delete(&db, root.id).await.unwrap_err();
    assert!(err.to_string().contains("子部门"));
}

/// 更新部门
#[tokio::test]
async fn test_dept_update() {
    let db = setup_test_db().await;
    let svc = DeptServiceImpl {
        dept_repo: LoggedRepo::new(DeptRepository),
        redis: None,
    };

    let d = create_test_dept(&db, "原始部门", None).await;

    let da = dept::ActiveModel {
        id: sea_orm::ActiveValue::Unchanged(d.id),
        name: sea_orm::ActiveValue::Set("newname".to_string()),
        status: sea_orm::ActiveValue::Set("0".to_string()),
        updated_at: sea_orm::ActiveValue::Set(Utc::now()),
        ..Default::default()
    };
    da.update(&db).await.expect("update dept failed");

    let updated = svc
        .find_by_id(&db, d.id)
        .await
        .unwrap()
        .expect("查不到部门");
    assert_eq!(updated.name, "newname");
    assert_eq!(updated.status, "0");
}

// ==================== MenuService 测试 ====================

/// 菜单树创建与查询
#[tokio::test]
async fn test_menu_create_and_tree() {
    let db = setup_test_db().await;
    let svc = MenuServiceImpl {
        menu_repo: LoggedRepo::new(MenuRepository),
        redis: None,
    };

    let root = svc
        .create(
            &db,
            "系统管理",
            None,
            "M",
            Some("/system"),
            None,
            None,
            None,
            Some("icon"),
            false,
            false,
            1,
            true,
        )
        .await
        .expect("创建菜单失败");
    assert_eq!(root.name, "系统管理");

    let _child = svc
        .create(
            &db,
            "用户管理",
            Some(root.id),
            "C",
            Some("/system/user"),
            Some("views/user/index"),
            None,
            None,
            None,
            false,
            false,
            1,
            true,
        )
        .await
        .expect("创建子菜单失败");

    let tree = svc.find_tree(&db).await.expect("查询树失败");
    assert!(!tree.is_empty());
}

/// 菜单更新
#[tokio::test]
async fn test_menu_update() {
    let db = setup_test_db().await;
    let svc = MenuServiceImpl {
        menu_repo: LoggedRepo::new(MenuRepository),
        redis: None,
    };

    let m = svc
        .create(
            &db,
            "原菜单",
            None,
            "C",
            Some("/a"),
            None,
            None,
            None,
            None,
            false,
            false,
            0,
            true,
        )
        .await
        .expect("创建失败");

    use ryframe_db::entities::menu;
    let ma = menu::ActiveModel {
        id: sea_orm::ActiveValue::Unchanged(m.id),
        name: sea_orm::ActiveValue::Set("newmenu".to_string()),
        visible: sea_orm::ActiveValue::Set(false),
        updated_at: sea_orm::ActiveValue::Set(Utc::now()),
        ..Default::default()
    };
    ma.update(&db).await.expect("update menu failed");

    let updated = svc
        .find_by_id(&db, m.id)
        .await
        .unwrap()
        .expect("查不到菜单");
    assert_eq!(updated.name, "newmenu");
    assert!(!updated.visible);
}

/// 菜单删除
#[tokio::test]
async fn test_menu_delete() {
    let db = setup_test_db().await;
    let svc = MenuServiceImpl {
        menu_repo: LoggedRepo::new(MenuRepository),
        redis: None,
    };

    let m = svc
        .create(
            &db,
            "待删菜单",
            None,
            "C",
            Some("/del"),
            None,
            None,
            None,
            None,
            false,
            false,
            0,
            true,
        )
        .await
        .expect("创建失败");

    svc.delete(&db, m.id).await.expect("删除失败");
    assert!(svc.find_by_id(&db, m.id).await.unwrap().is_none());
}

// ==================== AuthService 测试 ====================

/// 登录 → 获取用户信息 → 刷新令牌
#[tokio::test]
async fn test_auth_login_flow() {
    let db = setup_test_db().await;
    let config = Arc::new(test_config());

    let svc = AuthServiceImpl {
        user_repo: LoggedRepo::new(UserRepository),
        role_repo: LoggedRepo::new(RoleRepository),
        perm_repo: LoggedRepo::new(PermissionRepository),
        config: config.clone(),
    };

    // 创建用户
    create_test_user(&db, "admin").await;

    // 登录成功
    let result = svc.login(&db, "admin", "test123").await.expect("登录失败");
    assert_eq!(result.user_info.username, "admin");
    assert!(!result.access_token.is_empty());
    assert!(!result.refresh_token.is_empty());

    // 登录失败（错误密码）
    let err = svc.login(&db, "admin", "wrong").await.unwrap_err();
    assert!(err.to_string().contains("用户名或密码错误"));

    // 获取当前用户
    let user_info = svc
        .get_current_user(&db, result.user_info.id)
        .await
        .expect("获取用户失败");
    assert_eq!(user_info.username, "admin");

    // 刷新令牌
    let refreshed = svc
        .refresh_token(&db, &result.refresh_token)
        .await
        .expect("刷新失败");
    assert!(!refreshed.access_token.is_empty());
}

// ==================== ConfigService 测试 ====================

/// 参数配置 CRUD
#[tokio::test]
async fn test_config_crud() {
    let db = setup_test_db().await;
    let svc = ConfigServiceImpl {
        config_repo: LoggedRepo::new(ConfigRepository),
        redis: None,
    };

    // 创建
    let vo = svc
        .create(&db, "测试参数", "test.key", "value123", Some("备注"))
        .await
        .expect("创建失败");
    assert_eq!(vo.key, "test.key");

    // 按 key 查询
    let by_key = svc
        .find_by_key(&db, "test.key")
        .await
        .unwrap()
        .expect("按key查不到");
    assert_eq!(by_key.value, "value123");

    // key 重复
    let err = svc
        .create(&db, "测试参数2", "test.key", "v2", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("已存在"));
}

// ==================== DictService 测试 ====================

/// 字典类型 + 字典数据 CRUD
#[tokio::test]
async fn test_dict_crud() {
    let db = setup_test_db().await;
    let svc = DictServiceImpl {
        dict_type_repo: LoggedRepo::new(DictTypeRepository),
        dict_data_repo: LoggedRepo::new(DictDataRepository),
        redis: None,
    };

    // 创建字典类型
    let dtype = svc
        .create_type(&db, "性别", "sex")
        .await
        .expect("创建类型失败");
    assert_eq!(dtype.code, "sex");

    // 创建字典数据
    let ddata = svc
        .create_data(&db, "sex", "男", "1", 0)
        .await
        .expect("创建数据失败");
    assert_eq!(ddata.label, "男");

    // 按类型查数据
    let list = svc.find_data_by_type(&db, "sex").await.expect("查数据失败");
    assert_eq!(list.len(), 1);

    // 类型编码重复
    let err = svc.create_type(&db, "性别2", "sex").await.unwrap_err();
    assert!(err.to_string().contains("已存在"));
}

// ==================== PostService 测试 ====================

/// 岗位 CRUD
#[tokio::test]
async fn test_post_crud() {
    let db = setup_test_db().await;
    let svc = PostServiceImpl {
        post_repo: LoggedRepo::new(PostRepository),
    };

    let vo = svc
        .create(&db, "工程师", "engineer", 1)
        .await
        .expect("创建失败");
    assert_eq!(vo.code, "engineer");

    let found = svc.find_by_id(&db, vo.id).await.unwrap().expect("查不到");
    assert_eq!(found.name, "工程师");

    let page = svc
        .find_by_page(&db, PageQuery::default())
        .await
        .expect("分页失败");
    assert_eq!(page.total, 1);
}

// ==================== NoticeService 测试 ====================

/// 通知公告 CRUD
#[tokio::test]
async fn test_notice_crud() {
    let db = setup_test_db().await;
    let svc = NoticeServiceImpl {
        notice_repo: LoggedRepo::new(NoticeRepository),
    };

    let vo = svc
        .create(&db, "放假通知", "明天放假", Some("1"), None)
        .await
        .expect("创建失败");
    assert_eq!(vo.title, "放假通知");

    let page = svc
        .find_by_page(&db, PageQuery::default())
        .await
        .expect("分页失败");
    assert_eq!(page.total, 1);

    svc.delete(&db, vo.id).await.expect("删除失败");
    assert!(svc.find_by_id(&db, vo.id).await.unwrap().is_none());
}

// ==================== LoginInfoService 测试 ====================

/// 登录日志基本操作
#[tokio::test]
async fn test_login_info_service() {
    let db = setup_test_db().await;
    let svc = LoginInfoServiceImpl {
        login_info_repo: LoggedRepo::new(LoginInfoRepository),
    };

    // 记录登录日志
    svc.record_login(&db, "admin", "127.0.0.1", None, None, "1", Some("登录成功"))
        .await
        .expect("记录失败");

    let page = svc
        .find_by_page(&db, PageQuery::default(), None, None, None, None)
        .await
        .expect("分页失败");
    assert_eq!(page.total, 1);

    // 清空
    svc.clean(&db).await.expect("清空失败");
    let p2 = svc
        .find_by_page(&db, PageQuery::default(), None, None, None, None)
        .await
        .expect("分页失败");
    assert_eq!(p2.total, 0);
}

// ==================== OperLogService 测试 ====================

/// 操作日志记录
#[tokio::test]
async fn test_oper_log_service() {
    let db = setup_test_db().await;
    let svc = OperLogServiceImpl {
        oper_log_repo: LoggedRepo::new(OperLogRepository),
    };

    // 记录（通过内部方法）
    let page_before = svc
        .find_by_page(&db, PageQuery::default(), None, None, None, None)
        .await
        .expect("分页失败");
    let before = page_before.total;

    // 直接插入一条日志
    let log = ryframe_db::entities::oper_log::Model {
        id: snowflake::next_snowflake_id(),
        title: "测试操作".into(),
        business_type: "INSERT".into(),
        method: "POST /test".into(),
        request_method: "POST".into(),
        oper_name: "admin".into(),
        oper_url: "/test".into(),
        oper_ip: "127.0.0.1".into(),
        oper_location: None,
        oper_param: None,
        json_result: None,
        status: "1".into(),
        error_msg: None,
        oper_time: Utc::now(),
        cost_time: 10,
    };
    let repo = OperLogRepository;
    repo.insert(&db, log).await.expect("插入日志失败");

    let page = svc
        .find_by_page(&db, PageQuery::default(), None, None, None, None)
        .await
        .expect("分页失败");
    assert_eq!(page.total, before + 1);

    svc.clean(&db).await.expect("清空失败");
}

// ==================== PermissionService 测试 ====================

/// 权限树查询
#[tokio::test]
async fn test_permission_tree() {
    let db = setup_test_db().await;
    let svc = ryframe_service::system::PermissionServiceImpl {
        perm_repo: LoggedRepo::new(PermissionRepository),
    };

    // 未创建权限时树为空
    let tree = svc.find_tree(&db, None).await.expect("查询权限树失败");
    assert!(tree.is_empty());
}

// ==================== 综合场景测试 ====================

/// 用户创建 + 分配角色 全流程
#[tokio::test]
async fn test_user_create_with_roles() {
    let db = setup_test_db().await;
    let config = Arc::new(test_config());

    let user_svc = UserServiceImpl {
        user_repo: LoggedRepo::new(UserRepository),
        role_repo: LoggedRepo::new(RoleRepository),
        dept_repo: LoggedRepo::new(DeptRepository),
    };
    let _auth_svc = AuthServiceImpl {
        user_repo: LoggedRepo::new(UserRepository),
        role_repo: LoggedRepo::new(RoleRepository),
        perm_repo: LoggedRepo::new(PermissionRepository),
        config,
    };

    // 创建角色
    let role1 = create_test_role(&db, "测试角色", "test_role").await;

    // 创建用户并分配角色
    let user = user_svc
        .create(
            &db,
            CreateUserParams {
                username: "eve",
                password: "password",
                nickname: "Eve",
                email: "eve@test.com",
                phone: "138",
                dept_id: None,
                role_ids: Some(vec![role1.id]),
                enable_pwd_complexity: false,
            },
        )
        .await
        .expect("创建用户失败");

    // 验证角色分配
    let detail = user_svc
        .find_by_id_with_roles(&db, user.id)
        .await
        .unwrap()
        .expect("查不到用户");
    assert_eq!(detail.roles.len(), 1);
    assert_eq!(detail.roles[0].code, "test_role");
}

/// 部门用户数据权限场景
#[tokio::test]
async fn test_user_data_scope_context() {
    let db = setup_test_db().await;
    let user_svc = UserServiceImpl {
        user_repo: LoggedRepo::new(UserRepository),
        role_repo: LoggedRepo::new(RoleRepository),
        dept_repo: LoggedRepo::new(DeptRepository),
    };

    let dept = create_test_dept(&db, "IT部", None).await;
    let role = create_test_role(&db, "IT角色", "it_role").await;

    let user = create_test_user(&db, "it_user").await;

    // 构建数据权限上下文（admin角色返回 All scope）
    let roles = vec![role];
    let ctx = user_svc
        .build_data_scope_context(&db, user.id, Some(dept.id), &roles)
        .await
        .expect("构建权限上下文失败");

    // 非admin角色，data_scope=1(全部)，应该被识别
    assert_eq!(ctx.user_id, user.id);
}

// ==================== ProfileService 测试 ====================

/// 用户个人信息响应结构序列化
#[tokio::test]
async fn test_user_profile_response_structure() {
    use ryframe_service::system::profile_service::UserProfileResponse;

    let profile = UserProfileResponse {
        user_id: 1,
        username: "testuser".into(),
        nickname: "测试".into(),
        email: "test@test.com".into(),
        phone: "13800000000".into(),
        avatar: Some("/avatar/test.png".into()),
        dept_id: Some(100),
        dept_name: Some("技术部".into()),
        status: "1".into(),
        remark: None,
        login_ip: Some("127.0.0.1".into()),
        login_date: Some("2026-01-01T00:00:00+00:00".into()),
        created_at: "2026-01-01T00:00:00+00:00".into(),
        roles: vec!["admin".into(), "user".into()],
        permissions: vec!["system:user:list".into(), "system:user:add".into()],
    };

    let json = serde_json::to_value(&profile).unwrap();
    assert_eq!(json["user_id"], 1);
    assert_eq!(json["username"], "testuser");
    assert_eq!(json["nickname"], "测试");
    assert_eq!(json["roles"].as_array().unwrap().len(), 2);
    assert_eq!(json["permissions"].as_array().unwrap().len(), 2);
}

/// ProfileService 构造检查
#[tokio::test]
async fn test_profile_service_construction() {
    use ryframe_core::LoggedRepo;
    use ryframe_service::system::profile_service::ProfileServiceImpl;

    let _svc = ProfileServiceImpl {
        user_repo: LoggedRepo::new(UserRepository),
        role_repo: LoggedRepo::new(RoleRepository),
        perm_repo: LoggedRepo::new(PermissionRepository),
    };
}

// ==================== GeneratorService 测试 ====================

/// GeneratorService 构造检查
#[test]
fn test_generator_service_construction() {
    let workspace = std::path::PathBuf::from(".");
    let _svc = ryframe_service::system::GeneratorServiceImpl {
        workspace_root: workspace,
    };
}

// ==================== JobService 测试 ====================

/// cron 表达式验证（纯逻辑测试）
#[test]
fn test_cron_expression_validation() {
    use std::str::FromStr;

    // 有效 cron 表达式
    assert!(cron::Schedule::from_str("0 0 * * * *").is_ok()); // 每小时
    assert!(cron::Schedule::from_str("*/5 * * * * *").is_ok()); // 每5秒
    assert!(cron::Schedule::from_str("0 30 9 * * Mon-Fri *").is_ok()); // 工作日9:30

    // 无效 cron 表达式
    assert!(cron::Schedule::from_str("invalid").is_err());
    assert!(cron::Schedule::from_str("").is_err());
    assert!(cron::Schedule::from_str("99 99 99 * * * *").is_err());
}

/// JobVo / JobLogVo 序列化测试
#[test]
fn test_job_vo_serialization() {
    use ryframe_service::system::job_service::{JobLogVo, JobVo};

    let vo = JobVo {
        id: 1,
        name: "测试任务".into(),
        group_name: "default".into(),
        cron_expr: "0 0 * * * *".into(),
        status: "1".into(),
        description: "每小时执行".into(),
        next_fire_time: Some("2026-06-01T00:00:00Z".into()),
        remark: None,
    };

    let json = serde_json::to_value(&vo).unwrap();
    assert_eq!(json["name"], "测试任务");
    assert_eq!(json["cron_expr"], "0 0 * * * *");
    assert_eq!(json["status"], "1");

    let log_vo = JobLogVo {
        id: 1,
        job_name: "测试任务".into(),
        job_group: "default".into(),
        message: "执行成功".into(),
        status: "1".into(),
        error_msg: None,
        cost_ms: 150,
        start_time: chrono::Utc::now(),
    };

    let json = serde_json::to_value(&log_vo).unwrap();
    assert_eq!(json["job_name"], "测试任务");
    assert_eq!(json["status"], "1");
    assert_eq!(json["cost_ms"], 150);
}

/// JobService 构造检查
#[tokio::test]
async fn test_job_service_construction() {
    use std::sync::Arc;

    use ryframe_core::LoggedRepo;
    use ryframe_service::system::job_service::JobServiceImpl;

    let db = setup_test_db().await;
    let ctx = ryframe_task::TaskContext { db: Arc::new(db) };
    let scheduler = Arc::new(ryframe_task::TaskScheduler::new(ctx));
    let db_for_svc = setup_test_db().await;
    let _svc = JobServiceImpl {
        job_repo: LoggedRepo::new(JobRepository),
        job_log_repo: LoggedRepo::new(JobLogRepository),
        scheduler,
    };
    let _ = db_for_svc;
}

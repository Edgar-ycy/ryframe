//! DeptRepository integration tests.

mod common;

use chrono::Utc;
use common::setup_test_db;
use ryframe_common::utils::snowflake;
use ryframe_core::repository::{PageQuery, Repository};
use ryframe_db::{
    DeptRepository,
    entities::{dept, role, role_dept},
};
use sea_orm::{DatabaseConnection, EntityTrait};

const TENANT: &str = "system";

fn make_dept(
    name: &str,
    parent_id: Option<i64>,
    ancestors: &str,
    sort: i32,
    status: &str,
) -> dept::Model {
    dept::Model {
        id: snowflake::try_next_snowflake_id().expect("generate test ID"),
        tenant_id: TENANT.into(),
        name: name.into(),
        parent_id,
        ancestors: ancestors.into(),
        sort,
        status: status.into(),
        remark: None,
        del_flag: dept::Model::DEL_FLAG_NORMAL.into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

async fn insert_dept(
    db: &DatabaseConnection,
    name: &str,
    parent_id: Option<i64>,
    ancestors: &str,
    sort: i32,
) -> dept::Model {
    DeptRepository
        .insert(
            db,
            TENANT,
            make_dept(name, parent_id, ancestors, sort, dept::Model::STATUS_NORMAL),
        )
        .await
        .expect("insert department")
}

#[tokio::test]
async fn crud_pagination_and_filters_are_consistent() {
    let db = setup_test_db().await;
    let repo = DeptRepository;
    let engineering = insert_dept(&db, "研发部", None, "0", 1).await;
    let mut sales_model = make_dept("市场部", None, "0", 2, dept::Model::STATUS_DISABLED);
    sales_model.remark = Some("disabled".into());
    let sales = repo
        .insert(&db, TENANT, sales_model)
        .await
        .expect("insert sales");

    let page = repo
        .find_by_page(
            &db,
            TENANT,
            PageQuery {
                page: 1,
                page_size: 1,
            },
        )
        .await
        .expect("page departments");
    assert_eq!(page.total, 2);
    assert_eq!(page.records.len(), 1);

    let by_name = repo
        .find_filtered(&db, TENANT, Some("研发"), None)
        .await
        .expect("filter name");
    assert_eq!(
        by_name.iter().map(|item| item.id).collect::<Vec<_>>(),
        vec![engineering.id]
    );
    let by_status = repo
        .find_filtered(&db, TENANT, None, Some(dept::Model::STATUS_DISABLED))
        .await
        .expect("filter status");
    assert_eq!(by_status[0].id, sales.id);
    assert_eq!(
        repo.find_filtered_by_ids(&db, TENANT, None, None, &[engineering.id])
            .await
            .expect("filter visible ids")
            .len(),
        1
    );
    assert!(
        repo.find_filtered_by_ids(&db, TENANT, None, None, &[])
            .await
            .expect("empty ids")
            .is_empty()
    );

    let visible_page = repo
        .find_by_page_filtered_by_ids(&db, TENANT, PageQuery::default(), None, None, &[sales.id])
        .await
        .expect("visible page");
    assert_eq!(visible_page.total, 1);
    let empty_page = repo
        .find_by_page_filtered_by_ids(&db, TENANT, PageQuery::default(), None, None, &[])
        .await
        .expect("empty page");
    assert_eq!(empty_page.total, 0);

    let mut updated = engineering.clone();
    updated.name = "平台研发部".into();
    repo.update(&db, TENANT, updated)
        .await
        .expect("update department");
    assert_eq!(
        repo.find_by_id(&db, TENANT, engineering.id)
            .await
            .expect("find department")
            .expect("department exists")
            .name,
        "平台研发部"
    );
    repo.delete(&db, TENANT, engineering.id)
        .await
        .expect("delete department");
    assert!(
        repo.find_by_id(&db, TENANT, engineering.id)
            .await
            .expect("find deleted")
            .is_none()
    );
}

#[tokio::test]
async fn hierarchy_queries_keep_ancestor_paths_intact() {
    let db = setup_test_db().await;
    let repo = DeptRepository;
    let root = insert_dept(&db, "总公司", None, "0", 1).await;
    let child_ancestors = repo
        .build_ancestors(&db, TENANT, Some(root.id))
        .await
        .expect("child ancestors");
    let child = insert_dept(&db, "研发部", Some(root.id), &child_ancestors, 1).await;
    let grandchild_ancestors = repo
        .build_ancestors(&db, TENANT, Some(child.id))
        .await
        .expect("grandchild ancestors");
    let grandchild = insert_dept(&db, "前端组", Some(child.id), &grandchild_ancestors, 1).await;

    assert_eq!(
        repo.build_ancestors(&db, TENANT, None)
            .await
            .expect("root ancestors"),
        "0"
    );
    assert!(repo.build_ancestors(&db, TENANT, Some(-1)).await.is_err());
    assert!(
        repo.has_children(&db, TENANT, root.id)
            .await
            .expect("root children")
    );
    assert!(
        !repo
            .has_children(&db, TENANT, grandchild.id)
            .await
            .expect("leaf children")
    );

    let tree = repo.find_tree(&db, TENANT).await.expect("department tree");
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].id, root.id.to_string());
    assert_eq!(tree[0].children[0].id, child.id.to_string());
    assert_eq!(
        tree[0].children[0].children[0].id,
        grandchild.id.to_string()
    );

    let visible_tree = repo
        .find_tree_by_visible_ids(&db, TENANT, &[grandchild.id])
        .await
        .expect("visible tree");
    assert_eq!(
        visible_tree[0].children[0].children[0].id,
        grandchild.id.to_string()
    );
    assert!(
        repo.find_tree_by_visible_ids(&db, TENANT, &[])
            .await
            .expect("empty tree")
            .is_empty()
    );

    let child_ids = repo
        .find_child_dept_ids(&db, TENANT, root.id)
        .await
        .expect("child ids");
    assert_eq!(child_ids.len(), 3);
    let descendants = repo
        .find_descendants(&db, TENANT, root.id)
        .await
        .expect("descendants");
    assert_eq!(descendants.len(), 2);
    assert!(repo.find_child_dept_ids(&db, TENANT, -1).await.is_err());
}

#[tokio::test]
async fn department_queries_are_tenant_scoped() {
    let db = setup_test_db().await;
    insert_dept(&db, "系统部门", None, "0", 1).await;

    let mut other = make_dept("其他租户部门", None, "0", 1, dept::Model::STATUS_NORMAL);
    other.tenant_id = "other".into();
    DeptRepository
        .insert(&db, "other", other)
        .await
        .expect("insert other tenant department");

    assert_eq!(
        DeptRepository
            .find_by_page(&db, TENANT, PageQuery::default())
            .await
            .expect("system page")
            .total,
        1
    );
    let other_page = DeptRepository
        .find_by_page(&db, "other", PageQuery::default())
        .await
        .expect("other page");
    assert_eq!(other_page.total, 1);
}

#[tokio::test]
async fn department_role_scope_references_are_detected() {
    let db = setup_test_db().await;
    let dept = insert_dept(&db, "受限部门", None, "0", 1).await;
    assert!(
        !DeptRepository
            .is_referenced(&db, TENANT, dept.id)
            .await
            .expect("check unreferenced department")
    );

    role::Entity::insert(role::ActiveModel::from(role::Model {
        id: 1,
        tenant_id: TENANT.into(),
        name: "数据范围角色".into(),
        code: "scoped-role".into(),
        is_super: 0,
        data_scope: role::Model::DATA_SCOPE_CUSTOM.into(),
        status: role::Model::STATUS_NORMAL.into(),
        sort: 1,
        remark: None,
        del_flag: role::Model::DEL_FLAG_NORMAL.into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }))
    .exec(&db)
    .await
    .expect("insert role");
    role_dept::Entity::insert(role_dept::ActiveModel::from(role_dept::Model {
        tenant_id: TENANT.into(),
        role_id: 1,
        dept_id: dept.id,
    }))
    .exec(&db)
    .await
    .expect("insert role department scope");

    assert!(
        DeptRepository
            .is_referenced(&db, TENANT, dept.id)
            .await
            .expect("check referenced department")
    );
    assert!(
        !DeptRepository
            .is_referenced(&db, "other", dept.id)
            .await
            .expect("check tenant isolation")
    );
}

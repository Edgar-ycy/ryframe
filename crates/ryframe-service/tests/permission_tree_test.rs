use ryframe_db::entities::permission;
use ryframe_service::system::permission_service::build_perm_tree;

fn make_perm(id: i64, name: &str, code: &str, parent_id: Option<i64>) -> permission::Model {
    let now = chrono::Utc::now();
    permission::Model {
        id,
        tenant_id: "system".into(),
        name: name.into(),
        code: code.into(),
        parent_id,
        perm_type: "api".into(),
        icon: None,
        sort: 0,
        status: "1".into(),
        created_at: now,
        updated_at: now,
    }
}

#[test]
fn test_build_perm_tree() {
    let tree = build_perm_tree(&Vec::<&permission::Model>::new(), None);
    assert!(tree.is_empty());

    let p1 = make_perm(1, "用户列表", "system:user:list", None);
    let p2 = make_perm(2, "角色列表", "system:role:list", None);
    let perms = vec![&p1, &p2];
    let tree = build_perm_tree(&perms, None);
    assert_eq!(tree.len(), 2);

    let parent = make_perm(10, "系统管理", "system", None);
    let child1 = make_perm(11, "用户管理", "system:user", Some(10));
    let child2 = make_perm(12, "角色管理", "system:role", Some(10));
    let perms = vec![&parent, &child1, &child2];
    let tree = build_perm_tree(&perms, None);
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].children.len(), 2);
    assert_eq!(tree[0].children[0].name, "用户管理");
}

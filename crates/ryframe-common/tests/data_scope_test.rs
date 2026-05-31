use ryframe_common::{DataScope, DataScopeContext};

#[test]
fn test_data_scope_roundtrip() {
    for v in ["1", "2", "3", "4", "5"] {
        assert_eq!(DataScope::from_db_value(v).to_db_value(), v);
    }
    // 未知值默认为最严格
    assert_eq!(DataScope::from_db_value("0"), DataScope::SelfOnly);
    assert_eq!(DataScope::from_db_value("9"), DataScope::SelfOnly);
}

#[test]
fn test_sql_conditions() {
    // 超管无条件
    let admin = DataScopeContext::super_admin(42);
    assert!(admin.build_sql_condition("dept_id", "id").is_none());

    // 仅本人
    let ctx = DataScopeContext {
        scope: DataScope::SelfOnly,
        user_id: 100,
        dept_id: Some(10),
        ancestors: None,
        custom_dept_ids: vec![],
    };
    assert!(
        ctx.build_sql_condition("dept_id", "id")
            .unwrap()
            .contains("id = 100")
    );

    // 本部门
    let ctx = DataScopeContext {
        scope: DataScope::Dept,
        user_id: 1,
        dept_id: Some(5),
        ancestors: None,
        custom_dept_ids: vec![],
    };
    assert!(
        ctx.build_sql_condition("dept_id", "id")
            .unwrap()
            .contains("dept_id = 5")
    );

    // 无部门时看不到数据
    let ctx = DataScopeContext {
        scope: DataScope::Dept,
        user_id: 1,
        dept_id: None,
        ancestors: None,
        custom_dept_ids: vec![],
    };
    assert_eq!(ctx.build_sql_condition("dept_id", "id").unwrap(), "1 = 0");

    // 自定义部门
    let ctx = DataScopeContext {
        scope: DataScope::Custom,
        user_id: 1,
        dept_id: None,
        ancestors: None,
        custom_dept_ids: vec![10, 20, 30],
    };
    assert!(
        ctx.build_sql_condition("dept_id", "id")
            .unwrap()
            .contains("dept_id IN (10,20,30)")
    );

    // 自定义无部门时看不到数据
    let ctx = DataScopeContext {
        scope: DataScope::Custom,
        user_id: 1,
        dept_id: None,
        ancestors: None,
        custom_dept_ids: vec![],
    };
    assert_eq!(ctx.build_sql_condition("dept_id", "id").unwrap(), "1 = 0");
}

#[test]
fn test_merge_priority() {
    // 空合并默认 SelfOnly
    assert_eq!(DataScopeContext::merge(vec![]).scope, DataScope::SelfOnly);

    // 取最宽
    let scopes = vec![
        DataScopeContext {
            scope: DataScope::SelfOnly,
            user_id: 1,
            dept_id: Some(1),
            ancestors: None,
            custom_dept_ids: vec![],
        },
        DataScopeContext {
            scope: DataScope::Dept,
            user_id: 1,
            dept_id: Some(1),
            ancestors: None,
            custom_dept_ids: vec![],
        },
    ];
    assert_eq!(DataScopeContext::merge(scopes).scope, DataScope::Dept);

    // All 最优先
    let scopes = vec![
        DataScopeContext {
            scope: DataScope::Custom,
            user_id: 1,
            dept_id: Some(1),
            ancestors: None,
            custom_dept_ids: vec![10],
        },
        DataScopeContext {
            scope: DataScope::All,
            user_id: 1,
            dept_id: Some(1),
            ancestors: None,
            custom_dept_ids: vec![],
        },
    ];
    assert_eq!(DataScopeContext::merge(scopes).scope, DataScope::All);
}

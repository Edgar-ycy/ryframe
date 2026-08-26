use chrono::{DateTime, Utc};
use ryframe_application::{
    ports::{
        system::DeptRecord,
        users::{UserQueryDetailRecord, UserQueryRecord},
    },
    system::identity::UserDetailVo,
};

fn detail(dept_id: Option<i64>, department: Option<DeptRecord>) -> UserQueryDetailRecord {
    UserQueryDetailRecord {
        user: UserQueryRecord {
            id: 7,
            username: "tester".into(),
            nickname: "测试用户".into(),
            email: "tester@example.com".into(),
            phone: "13800000000".into(),
            avatar: None,
            status: "1".into(),
            dept_id,
            dept_name: department
                .as_ref()
                .map(|department| department.name.clone()),
            remark: None,
            created_at: DateTime::<Utc>::UNIX_EPOCH,
        },
        department,
        roles: Vec::new(),
    }
}

fn department() -> DeptRecord {
    DeptRecord {
        id: 9,
        name: "研发部".into(),
        parent_id: Some(1),
        ancestors: "0,1".into(),
        sort: 10,
        status: "1".into(),
        remark: Some("核心研发".into()),
        created_at: DateTime::<Utc>::UNIX_EPOCH,
        updated_at: DateTime::<Utc>::UNIX_EPOCH,
    }
}

#[test]
fn user_detail_includes_the_complete_department_projection() {
    let detail = UserDetailVo::from(detail(Some(9), Some(department())));
    let department = detail.department.expect("关联部门必须存在");

    assert_eq!(detail.user.dept_id.as_deref(), Some("9"));
    assert_eq!(detail.user.dept_name.as_deref(), Some("研发部"));
    assert_eq!(department.id, "9");
    assert_eq!(department.parent_id.as_deref(), Some("1"));
    assert_eq!(department.ancestors, "0,1");
    assert_eq!(department.sort, 10);
    assert_eq!(department.status, "1");
    assert_eq!(department.remark.as_deref(), Some("核心研发"));
}

#[test]
fn user_detail_keeps_department_null_for_absent_or_missing_relations() {
    let absent = UserDetailVo::from(detail(None, None));
    assert!(absent.department.is_none());
    assert!(absent.user.dept_id.is_none());

    let missing = UserDetailVo::from(detail(Some(99), None));
    assert!(missing.department.is_none());
    assert_eq!(missing.user.dept_id.as_deref(), Some("99"));
    assert!(missing.user.dept_name.is_none());
}

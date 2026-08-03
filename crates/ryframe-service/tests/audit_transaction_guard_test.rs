struct ServiceAuditContract {
    name: &'static str,
    source: &'static str,
    expected_transactions: usize,
    write_methods: &'static [&'static str],
}

const SERVICES: &[ServiceAuditContract] = &[
    ServiceAuditContract {
        name: "参数配置",
        source: include_str!("../src/system/config_service.rs"),
        expected_transactions: 4,
        write_methods: &["create", "update", "delete", "clear_cache"],
    },
    ServiceAuditContract {
        name: "部门",
        source: include_str!("../src/system/dept_service/commands.rs"),
        expected_transactions: 4,
        write_methods: &["create", "update", "delete"],
    },
    ServiceAuditContract {
        name: "菜单",
        source: include_str!("../src/system/menu_service.rs"),
        expected_transactions: 3,
        write_methods: &["create", "update", "delete"],
    },
    ServiceAuditContract {
        name: "权限",
        source: include_str!("../src/system/permission_service.rs"),
        expected_transactions: 4,
        write_methods: &["create", "update", "delete", "sync_route_permissions"],
    },
    ServiceAuditContract {
        name: "字典",
        source: include_str!("../src/system/dict_service.rs"),
        expected_transactions: 6,
        write_methods: &[
            "create_type",
            "update_type",
            "delete_type",
            "create_data",
            "update_data",
            "delete_data",
        ],
    },
    ServiceAuditContract {
        name: "岗位",
        source: include_str!("../src/system/post_service.rs"),
        expected_transactions: 3,
        write_methods: &["create", "update", "delete"],
    },
    ServiceAuditContract {
        name: "公告",
        source: include_str!("../src/system/notice_service.rs"),
        expected_transactions: 3,
        write_methods: &["create", "update", "delete"],
    },
    ServiceAuditContract {
        name: "登录日志",
        source: include_str!("../src/system/login_info_service.rs"),
        expected_transactions: 1,
        write_methods: &["clean"],
    },
    ServiceAuditContract {
        name: "操作日志",
        source: include_str!("../src/system/oper_log_service.rs"),
        expected_transactions: 1,
        write_methods: &["clean"],
    },
    ServiceAuditContract {
        name: "角色",
        source: include_str!("../src/system/role_service.rs"),
        expected_transactions: 5,
        write_methods: &[
            "delete_many",
            "create",
            "update",
            "delete",
            "assign_permissions",
            "replace_data_scope",
        ],
    },
    ServiceAuditContract {
        name: "租户",
        source: include_str!("../src/system/tenant_service.rs"),
        expected_transactions: 3,
        write_methods: &["create", "update", "update_status"],
    },
    ServiceAuditContract {
        name: "个人资料",
        source: include_str!("../src/system/profile_service.rs"),
        expected_transactions: 4,
        write_methods: &[
            "update_profile",
            "change_password",
            "update_avatar",
            "schedule_unreferenced_avatar_cleanup",
        ],
    },
    ServiceAuditContract {
        name: "用户命令",
        source: include_str!("../src/system/user_service/commands.rs"),
        expected_transactions: 5,
        write_methods: &["create", "update", "update_status", "delete_many", "delete"],
    },
    ServiceAuditContract {
        name: "密码重置",
        source: include_str!("../src/system/user_service/password_reset.rs"),
        expected_transactions: 2,
        write_methods: &["request_password_reset", "complete_password_reset_request"],
    },
    ServiceAuditContract {
        name: "用户角色",
        source: include_str!("../src/system/user_service/roles.rs"),
        expected_transactions: 1,
        write_methods: &["replace_roles"],
    },
];

struct ExtendedServiceAuditContract {
    name: &'static str,
    source: &'static str,
    expected_transactions: usize,
    expected_audit_commits: usize,
    expected_internal_commits: usize,
    expected_rollback_only_transactions: usize,
    write_methods: &'static [&'static str],
}

const EXTENDED_SERVICES: &[ExtendedServiceAuditContract] = &[
    ExtendedServiceAuditContract {
        name: "站内消息",
        source: include_str!("../src/system/message_service.rs"),
        expected_transactions: 7,
        expected_audit_commits: 7,
        expected_internal_commits: 0,
        expected_rollback_only_transactions: 0,
        write_methods: &[
            "publish",
            "acknowledge_for_identity",
            "acknowledge",
            "mark_read",
            "mark_all_read",
            "dispatch",
            "delete_expired",
        ],
    },
    ExtendedServiceAuditContract {
        name: "异步导出",
        source: include_str!("../src/system/export_service.rs"),
        expected_transactions: 8,
        expected_audit_commits: 7,
        expected_internal_commits: 0,
        expected_rollback_only_transactions: 1,
        write_methods: &[
            "request",
            "cancel_for_requester",
            "cleanup_expired",
            "execute_background_job",
            "record_execution_failure",
        ],
    },
    ExtendedServiceAuditContract {
        name: "文件上传",
        source: concat!(
            include_str!("../src/system/file_service.rs"),
            include_str!("../src/system/file_service/upload_reservation.rs")
        ),
        expected_transactions: 3,
        expected_audit_commits: 2,
        expected_internal_commits: 1,
        expected_rollback_only_transactions: 0,
        write_methods: &[
            "upload_single",
            "upload_avatar",
            "reconcile_upload_reservations",
        ],
    },
];

#[test]
fn 写服务的事务必须通过审计提交守卫() {
    for service in SERVICES {
        let begin_count = service.source.matches(".begin()").count();
        let audit_commit_count = service
            .source
            .matches("crate::commit_current_audit(transaction)")
            .count();

        assert_eq!(
            begin_count, service.expected_transactions,
            "{}服务新增了未纳入审计清单的事务，请同步更新守卫",
            service.name
        );
        assert_eq!(
            audit_commit_count, begin_count,
            "{}服务存在没有原子写入 audit.operation Outbox 的事务",
            service.name
        );
        assert!(
            !service.source.contains(".commit()"),
            "{}服务禁止绕过 commit_current_audit 裸提交事务",
            service.name
        );

        for method in service.write_methods {
            assert!(
                service.source.contains(&format!("pub async fn {method}")),
                "{}服务的写方法 {method} 已改名或移除，请重新审查事务覆盖",
                service.name
            );
        }
    }
}

#[test]
fn 消息导出与文件事务必须通过分层审计提交守卫() {
    for service in EXTENDED_SERVICES {
        let begin_count = service.source.matches(".begin()").count();
        let audit_commit_count = service
            .source
            .matches("crate::commit_current_audit(transaction)")
            .count();
        let internal_commit_count = service
            .source
            .matches(".commit_upload_reservation(transaction)")
            .count();

        assert_eq!(
            begin_count, service.expected_transactions,
            "{}服务新增了未纳入分层审计清单的事务，请同步更新守卫",
            service.name
        );
        assert_eq!(
            audit_commit_count, service.expected_audit_commits,
            "{}服务的审计事务数量与清单不一致",
            service.name
        );
        assert_eq!(
            internal_commit_count, service.expected_internal_commits,
            "{}服务的内部协调事务数量与清单不一致",
            service.name
        );
        assert_eq!(
            audit_commit_count
                + internal_commit_count
                + service.expected_rollback_only_transactions,
            begin_count,
            "{}服务存在未分类的事务",
            service.name
        );
        assert!(
            !service.source.contains(".commit()"),
            "{}服务禁止裸提交事务",
            service.name
        );

        for method in service.write_methods {
            assert!(
                service.source.contains(&format!("pub async fn {method}")),
                "{}服务的写方法 {method} 已改名或移除，请重新审查事务覆盖",
                service.name
            );
        }
    }
}

#[test]
fn 上传预留与导出补偿不得提前绑定成功审计() {
    let upload_source = include_str!("../src/system/file_service/upload_reservation.rs");
    let reserve_section = upload_source
        .split_once("pub(super) async fn reserve_upload")
        .and_then(|(_, tail)| tail.split_once("pub(super) async fn recover_in_progress_upload"))
        .map(|(section, _)| section)
        .expect("必须能够定位上传预留实现");
    assert!(reserve_section.contains(".commit_upload_reservation(transaction)"));
    assert!(!reserve_section.contains("commit_current_audit"));

    let export_source = include_str!("../src/system/export_service.rs");
    let compensation_section = export_source
        .split_once("async fn compensate_uncommitted_object")
        .map(|(_, section)| section)
        .expect("必须能够定位导出对象补偿实现");
    assert!(compensation_section.contains("transaction.rollback()"));
    assert!(!compensation_section.contains("commit_current_audit"));

    let file_repository = include_str!("../../ryframe-db/src/repositories/file_repo.rs");
    assert!(file_repository.contains("pub async fn commit_upload_reservation("));
    assert_eq!(file_repository.matches(".commit()").count(), 1);
}

#[test]
fn 租户初始化仓储只能加入调用方事务() {
    let source = include_str!("../../ryframe-db/src/repositories/tenant_provisioning_repo.rs");

    assert!(source.contains("pub async fn provision_in_transaction("));
    assert!(!source.contains("pub async fn provision("));
    assert!(!source.contains(".begin()"));
    assert!(!source.contains(".commit()"));
}

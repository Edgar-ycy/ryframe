use ryframe_db::repositories::post_export_repo::{PostExportFilter, post_export_batch_query};
use ryframe_kernel::ExportCursorWindow;
use sea_orm::{DatabaseBackend, QueryTrait};

#[test]
fn batch_query_keeps_tenant_soft_delete_filters_and_stable_cursor() {
    let filter = PostExportFilter {
        name: Some("平台"),
        code: Some("platform"),
        status: Some("1"),
    };
    let statement = post_export_batch_query(
        "tenant-a",
        &filter,
        ExportCursorWindow::new(Some(10), 99, 20),
    )
    .build(DatabaseBackend::MySql);

    assert!(statement.sql.contains("`sys_post`.`tenant_id` = ?"));
    assert!(statement.sql.contains("`sys_post`.`del_flag` = ?"));
    assert!(statement.sql.contains("`sys_post`.`name` LIKE ?"));
    assert!(statement.sql.contains("`sys_post`.`code` LIKE ?"));
    assert!(statement.sql.contains("`sys_post`.`status` = ?"));
    assert!(statement.sql.contains("`sys_post`.`id` <= ?"));
    assert!(statement.sql.contains("`sys_post`.`id` > ?"));
    assert!(statement.sql.contains("ORDER BY `sys_post`.`id` ASC"));
    assert!(statement.sql.ends_with("LIMIT ?"));

    let values = format!("{:?}", statement.values);
    for expected in [
        "tenant-a",
        "0",
        "%平台%",
        "%platform%",
        "1",
        "99",
        "10",
        "20",
    ] {
        assert!(
            values.contains(expected),
            "查询缺少绑定值 {expected}: {values}"
        );
    }
}

#[test]
fn batch_query_ignores_empty_optional_filters() {
    let statement = post_export_batch_query(
        "tenant-a",
        &PostExportFilter {
            name: Some(""),
            code: None,
            status: Some(""),
        },
        ExportCursorWindow::new(None, 99, 20),
    )
    .build(DatabaseBackend::MySql);

    assert!(!statement.sql.contains("`sys_post`.`name` LIKE ?"));
    assert!(!statement.sql.contains("`sys_post`.`code` LIKE ?"));
    assert!(!statement.sql.contains("`sys_post`.`status` = ?"));
    assert!(!statement.sql.contains("`sys_post`.`id` > ?"));
}

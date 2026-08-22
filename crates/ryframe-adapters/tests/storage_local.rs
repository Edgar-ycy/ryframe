use std::sync::Arc;

use ryframe_adapters::storage::{
    LocalObjectStorage, MAX_OBJECT_LIST_PAGE_SIZE, ObjectStorage, ScopedObjectStorage,
};

#[tokio::test]
async fn put_file_streams_through_private_staging() {
    let directory = tempfile::tempdir().expect("创建测试目录");
    let source = directory.path().join("source.xlsx");
    let content = b"xlsx artifact from disk";
    tokio::fs::write(&source, content)
        .await
        .expect("写入源文件");
    let storage = LocalObjectStorage::new(directory.path().join("objects"));

    storage
        .put_file(
            "exports",
            "scope/jobs/result.xlsx",
            &source,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            None,
        )
        .await
        .expect("流式上传文件");

    let stored = storage
        .get("exports", "scope/jobs/result.xlsx")
        .await
        .expect("读取上传结果");
    assert_eq!(stored, content);
}

#[tokio::test]
async fn bounded_read_rejects_oversized_control_object() {
    let directory = tempfile::tempdir().expect("创建测试目录");
    let storage = LocalObjectStorage::new(directory.path().join("objects"));
    storage
        .put("exports", "scope/.ryframe-owner", b"owner", "text/plain")
        .await
        .expect("写入控制对象");

    assert_eq!(
        storage
            .get_bounded("exports", "scope/.ryframe-owner", 5)
            .await
            .expect("上限内读取成功"),
        b"owner"
    );
    assert!(
        storage
            .get_bounded("exports", "scope/.ryframe-owner", 4)
            .await
            .is_err()
    );
    assert!(
        storage
            .get_bounded("exports", "scope/.ryframe-owner", 0)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn exact_prefix_listing_is_paginated_and_bounded() {
    let directory = tempfile::tempdir().expect("创建测试目录");
    let storage = LocalObjectStorage::new(directory.path().join("objects"));
    for key in [
        "scope/a.txt",
        "scope/b.txt",
        "scope/nested/c.txt",
        "scope-other/outside.txt",
    ] {
        storage
            .put("exports", key, key.as_bytes(), "text/plain")
            .await
            .expect("写入测试对象");
    }

    let first = storage
        .list_page("exports", "scope/", None, 2)
        .await
        .expect("列举第一页");
    assert_eq!(first.keys, ["scope/a.txt", "scope/b.txt"]);
    let second = storage
        .list_page("exports", "scope/", first.next_cursor.as_deref(), 2)
        .await
        .expect("列举第二页");
    assert_eq!(second.keys, ["scope/nested/c.txt"]);
    assert!(second.next_cursor.is_none());

    assert!(
        storage
            .list_page("exports", "scope", None, 1)
            .await
            .is_err()
    );
    assert!(storage.list_page("exports", "", None, 1).await.is_err());
    assert!(
        storage
            .list_page("exports", "scope/", None, 0)
            .await
            .is_err()
    );
    assert!(
        storage
            .list_page("exports", "scope/", None, MAX_OBJECT_LIST_PAGE_SIZE + 1)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn prefix_cleanup_never_deletes_a_neighboring_prefix() {
    let directory = tempfile::tempdir().expect("创建测试目录");
    let storage = LocalObjectStorage::new(directory.path().join("objects"));
    for key in ["scope/a.txt", "scope/b.txt", "scope/c.txt"] {
        storage
            .put("exports", key, b"inside", "text/plain")
            .await
            .expect("写入前缀内对象");
    }
    storage
        .put("exports", "scope-other/keep.txt", b"outside", "text/plain")
        .await
        .expect("写入相邻前缀对象");

    let first = storage
        .delete_prefix_batch("exports", "scope/", 2)
        .await
        .expect("清理第一批");
    assert_eq!(first.deleted_count, 2);
    assert!(first.may_have_more);
    let second = storage
        .delete_prefix_batch("exports", "scope/", 2)
        .await
        .expect("清理第二批");
    assert_eq!(second.deleted_count, 1);
    assert!(!second.may_have_more);
    assert!(
        storage
            .prefix_is_empty("exports", "scope/")
            .await
            .expect("验证前缀为空")
    );
    assert!(
        storage
            .exists("exports", "scope-other/keep.txt")
            .await
            .expect("检查相邻前缀对象")
    );
}

#[tokio::test]
async fn scoped_storage_applies_scope_once_and_rejects_physical_keys() {
    let directory = tempfile::tempdir().expect("创建测试目录");
    let inner = Arc::new(LocalObjectStorage::new(directory.path().join("objects")));
    let storage = ScopedObjectStorage::new(inner.clone(), "test-a");
    storage
        .put(
            "exports",
            "jobs/result.xlsx",
            b"result",
            "application/octet-stream",
        )
        .await
        .expect("写入逻辑对象键");
    assert!(
        inner
            .exists("exports", "test-a/jobs/result.xlsx")
            .await
            .expect("检查物理对象键")
    );
    assert!(
        storage
            .put(
                "exports",
                "test-a/jobs/result.xlsx",
                b"duplicate",
                "application/octet-stream"
            )
            .await
            .is_err()
    );
    assert!(
        storage
            .put("exports", ".ryframe-owner", b"owner", "text/plain")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn scoped_storage_rejects_fuzzy_and_pre_scoped_prefixes() {
    let directory = tempfile::tempdir().expect("创建测试目录");
    let inner = Arc::new(LocalObjectStorage::new(directory.path().join("objects")));
    let storage = ScopedObjectStorage::new(inner, "test-a");

    assert!(storage.prefix_is_empty("exports", "").await.is_ok());
    assert!(storage.prefix_is_empty("exports", "jobs/").await.is_ok());
    assert!(storage.prefix_is_empty("exports", "jobs").await.is_err());
    assert!(
        storage
            .prefix_is_empty("exports", "test-a/jobs/")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn scoped_root_cleanup_is_confined_to_current_scope() {
    let directory = tempfile::tempdir().expect("创建测试目录");
    let inner: Arc<dyn ObjectStorage> =
        Arc::new(LocalObjectStorage::new(directory.path().join("objects")));
    let scope_a = ScopedObjectStorage::new(Arc::clone(&inner), "test-a");
    let scope_b = ScopedObjectStorage::new(inner, "test-b");
    scope_a
        .ensure_bucket("exports")
        .await
        .expect("初始化 scope A");
    scope_b
        .ensure_bucket("exports")
        .await
        .expect("初始化 scope B");
    scope_a
        .put("exports", "jobs/a.xlsx", b"a", "application/octet-stream")
        .await
        .expect("写入 scope A");
    scope_b
        .put("exports", "jobs/b.xlsx", b"b", "application/octet-stream")
        .await
        .expect("写入 scope B");

    loop {
        let batch = scope_a
            .delete_prefix_batch("exports", "", 1)
            .await
            .expect("分批清理 scope A");
        if !batch.may_have_more {
            break;
        }
    }

    assert!(
        scope_a
            .prefix_is_empty("exports", "")
            .await
            .expect("验证 scope A 为空")
    );
    assert!(
        scope_b
            .exists("exports", "jobs/b.xlsx")
            .await
            .expect("检查 scope B 对象")
    );
    scope_b
        .verify_ownership_marker("exports")
        .await
        .expect("scope B 所有权标记必须保留");
    scope_a
        .ensure_bucket("exports")
        .await
        .expect("重建 scope A");
    scope_a
        .verify_ownership_marker("exports")
        .await
        .expect("scope A 所有权标记必须可重新建立");
}

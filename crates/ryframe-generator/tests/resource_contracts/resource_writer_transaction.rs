use std::fs;

use ryframe_generator::ResourceError;
use sha2::{Digest, Sha256};

#[path = "../../src/resource/writer/transaction.rs"]
#[allow(dead_code)]
mod transaction;

use transaction::{
    ExpectedFile, InstalledFile, persist_recovery_directories, rollback, verify_expected_file,
};

fn content_hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[test]
fn compare_and_swap_detects_concurrent_create_and_mutation() {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let path = directory.path().join("managed.rs");
    verify_expected_file(&path, &ExpectedFile::Absent).expect("目标最初应不存在");
    fs::write(&path, "并发创建").expect("应模拟并发创建");
    let created = verify_expected_file(&path, &ExpectedFile::Absent)
        .expect_err("不得覆盖并发创建的目标")
        .to_string();
    assert!(created.contains("compare-and-swap"));

    let expected = ExpectedFile::ContentHash(content_hash("原内容".as_bytes()));
    fs::write(&path, "原内容").expect("应写入预期内容");
    verify_expected_file(&path, &expected).expect("原内容应匹配");
    fs::write(&path, "并发修改").expect("应模拟并发修改");
    let changed = verify_expected_file(&path, &expected)
        .expect_err("不得覆盖并发修改")
        .to_string();
    assert!(changed.contains("compare-and-swap"));

    let manifest = directory.path().join(".ownership.toml");
    fs::write(&manifest, "version = 1\n").expect("应写入 manifest");
    let expected = ExpectedFile::ExactBytes(b"version = 1\n".to_vec());
    fs::write(&manifest, "version = 2\n").expect("应模拟 manifest 并发修改");
    let changed = verify_expected_file(&manifest, &expected)
        .expect_err("不得覆盖并发修改的 manifest")
        .to_string();
    assert!(changed.contains("ownership manifest"));
}

#[test]
fn rollback_failure_is_observable_and_preserves_concurrent_content() {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let target = directory.path().join("managed.rs");
    let backup = directory.path().join("backup.rs");
    fs::write(&target, "并发新内容").expect("应写入并发内容");
    fs::write(&backup, "旧生成内容").expect("应写入备份");
    let installed = [InstalledFile {
        path: target.clone(),
        content_hash: content_hash("生成器安装内容".as_bytes()),
    }];
    let error = rollback(&installed, &[(target.clone(), backup.clone())])
        .expect_err("回滚冲突必须可观测")
        .to_string();
    assert!(error.contains("拒绝删除已被并发修改"));
    assert!(error.contains("旧文件仍在"));
    assert_eq!(fs::read_to_string(target).unwrap(), "并发新内容");
    assert_eq!(fs::read_to_string(backup).unwrap(), "旧生成内容");
}

#[test]
fn incomplete_rollback_keeps_backup_directories_after_tempdir_drop() {
    let backend = tempfile::tempdir().expect("应创建后端临时事务目录");
    let frontend = tempfile::tempdir().expect("应创建前端临时事务目录");
    let backend_backup = backend.path().join(".backup/managed.rs");
    let frontend_backup = frontend.path().join(".backup/managed.ts");
    fs::create_dir_all(backend_backup.parent().unwrap()).expect("应创建后端备份目录");
    fs::create_dir_all(frontend_backup.parent().unwrap()).expect("应创建前端备份目录");
    fs::write(&backend_backup, "后端旧内容").expect("应写入后端备份");
    fs::write(&frontend_backup, "前端旧内容").expect("应写入前端备份");

    let directories = persist_recovery_directories(backend, Some(frontend));

    assert_eq!(directories.len(), 2);
    assert_eq!(fs::read_to_string(&backend_backup).unwrap(), "后端旧内容");
    assert_eq!(fs::read_to_string(&frontend_backup).unwrap(), "前端旧内容");
    assert!(directories.iter().all(|directory| directory.is_dir()));

    fs::remove_dir_all(directories[0].parent().unwrap()).expect("应清理后端测试现场");
    fs::remove_dir_all(directories[1].parent().unwrap()).expect("应清理前端测试现场");
}

use std::{fs, path::PathBuf};

use ryframe_db::migration::mysql_snapshot_sql;

#[test]
fn deployment_sql_has_credentials_constraints_and_complete_ledger() {
    let hash = "$argon2id$v=19$m=65536,t=3,p=4$O8qRRhiIVYjCHpUuwGWTSA$OO+ik8t1+N5a4PSipMbB71W/pfc3roAbq6mdIAgV1bA";
    let hashes = ryframe_db::migration::BootstrapHashes::new(hash.into(), hash.into()).unwrap();
    let sql = ryframe_db::migration::mysql_initialization_sql(&hashes);
    assert!(!sql.contains("${RYFRAME_"));
    assert!(!sql.contains("CREATE TABLE IF NOT EXISTS"));
    assert!(sql.contains("CHECK (`valid` = 1)"));
    assert!(sql.contains("ADD CONSTRAINT `fk_user_avatar_file`"));
    assert!(sql.contains("`password_change_required`"));
    let notice = sql.find("CREATE TABLE `sys_notice`").unwrap();
    let revision = sql
        .find("ALTER TABLE `sys_notice` ADD COLUMN `revision` BIGINT UNSIGNED NOT NULL DEFAULT 1")
        .unwrap();
    let seed = sql.find("INSERT INTO `sys_tenant`").unwrap();
    let ledger = sql.find("CREATE TABLE `seaql_migrations`").unwrap();
    assert!(notice < revision && revision < seed && seed < ledger);
    assert!(sql.find("INSERT INTO `sys_user`").unwrap() < ledger);
    for name in ryframe_db::migration::expected_migration_names() {
        assert!(sql[ledger..].contains(name));
    }
}

#[test]
fn credential_hash_rejects_sql_injection_and_unexpanded_placeholders() {
    for value in [
        "${RYFRAME_ADMIN_PASSWORD_HASH}",
        "$argon2id$'",
        "$argon2id$\\",
        "",
    ] {
        assert!(ryframe_db::migration::BootstrapHashes::new(value.into(), value.into()).is_err());
    }
}

#[test]
fn snapshot_places_avatar_foreign_key_after_file_table_before_seeds() {
    let sql = mysql_snapshot_sql();
    let file_table = sql.find("CREATE TABLE IF NOT EXISTS `sys_file`").unwrap();
    let avatar_constraint = sql.find("ADD CONSTRAINT `fk_user_avatar_file`").unwrap();
    let first_seed = sql.find("INSERT INTO `sys_tenant`").unwrap();
    assert!(file_table < avatar_constraint && avatar_constraint < first_seed);
}

#[test]
fn mysql_snapshot_matches_and_exports_candidate() {
    let rendered = mysql_snapshot_sql();
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("DB crate 应位于 Workspace/crates")
        .to_path_buf();
    let committed = fs::read_to_string(workspace.join("sql/ryframe_config.sql"))
        .expect("应读取已提交 MySQL 快照");
    assert_eq!(rendered, committed, "MySQL 快照必须与当前代码一致");
}

use std::{env, fs, path::PathBuf};

use ryframe_db::migration::mysql_snapshot_sql;

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

    if let Some(output) = env::var_os("RYFRAME_VERIFY_MYSQL_SNAPSHOT_OUTPUT") {
        fs::write(output, rendered).expect("应写入候选 MySQL 快照");
    }
}

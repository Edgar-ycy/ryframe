use std::{env, fs, path::PathBuf};

use ryframe_api::openapi::{ApiDoc, render_openapi_json};
use utoipa::OpenApi;

#[test]
fn openapi_snapshot_matches_and_exports_candidate() {
    let rendered = render_openapi_json(&ApiDoc::openapi()).expect("OpenAPI 应可稳定渲染");
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("API crate 应位于 Workspace/crates")
        .to_path_buf();
    let committed = fs::read_to_string(workspace.join("openapi/openapi.json"))
        .expect("应读取已提交 OpenAPI 快照");
    assert_eq!(rendered, committed, "OpenAPI 快照必须与当前代码一致");

    if let Some(output) = env::var_os("RYFRAME_VERIFY_OPENAPI_SNAPSHOT_OUTPUT") {
        fs::write(output, rendered).expect("应写入候选 OpenAPI 快照");
    }
}

use std::{env, fs, path::PathBuf};

use ryframe_api::openapi::render_openapi_json;

#[test]
fn combined_openapi_matches_the_committed_snapshot() {
    let rendered = render_openapi_json(&ryframe_business_api::combined_openapi())
        .expect("合并 OpenAPI 应可稳定渲染");
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("业务 API crate 应位于 Workspace/crates/business")
        .to_path_buf();
    let committed = fs::read_to_string(workspace.join("openapi/openapi.json"))
        .expect("应读取已提交 OpenAPI 快照");
    assert_eq!(rendered, committed, "合并 OpenAPI 快照必须与当前代码一致");

    if let Some(output) = env::var_os("RYFRAME_VERIFY_OPENAPI_SNAPSHOT_OUTPUT") {
        fs::write(output, rendered).expect("应写入候选 OpenAPI 快照");
    }
}

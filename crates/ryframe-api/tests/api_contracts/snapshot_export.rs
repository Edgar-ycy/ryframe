use std::{fs, path::PathBuf};

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
    let framework: serde_json::Value = serde_json::from_str(&rendered).expect("框架契约应有效");
    let combined: serde_json::Value = serde_json::from_str(&committed).expect("合并契约应有效");
    for (path, operation) in framework["paths"].as_object().expect("框架契约应包含路径") {
        assert_eq!(
            combined["paths"].get(path),
            Some(operation),
            "合并契约必须完整保留框架路径 {path}"
        );
    }
}

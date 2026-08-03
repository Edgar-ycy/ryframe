use std::{fs, path::Path};

fn visit_rust_sources(root: &Path, callback: &mut impl FnMut(&Path, &str)) {
    for entry in fs::read_dir(root).expect("必须能够读取源码目录") {
        let path = entry.expect("必须能够读取源码目录项").path();
        if path.is_dir() {
            visit_rust_sources(&path, callback);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let source = fs::read_to_string(&path).expect("Rust 源码必须是 UTF-8");
            callback(&path, &source);
        }
    }
}

fn contains_identifier(source: &str, expected: &str) -> bool {
    source
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .any(|identifier| identifier == expected)
}

#[test]
fn service_and_repository_do_not_accept_unvalidated_pagination() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("核心 crate 必须位于工作区 crates 目录中");
    let removed_query_type = concat!("Page", "Query");

    for relative in ["crates/ryframe-service/src", "crates/ryframe-db/src"] {
        visit_rust_sources(&workspace.join(relative), &mut |path, source| {
            assert!(
                !contains_identifier(source, removed_query_type),
                "{} 仍依赖旧的未校验分页类型",
                path.display()
            );
            assert!(
                !source.contains("page: u64") && !source.contains("page_size: u64"),
                "{} 在服务或仓储边界接收原始分页数字",
                path.display()
            );
            if source.contains("crate::pagination::paginate(") {
                assert!(
                    source.contains("ValidatedPageQuery"),
                    "{} 调用统一分页器时未声明 ValidatedPageQuery",
                    path.display()
                );
            }
        });
    }

    let api_macro = fs::read_to_string(workspace.join("crates/ryframe-api/src/macros.rs"))
        .expect("必须能够读取 API 查询宏");
    assert!(api_macro.contains("ValidatedPageQuery::from_optional("));

    for relative in [
        "crates/ryframe-api/src/dto/job_dto.rs",
        "crates/ryframe-api/src/handlers/generator_handler.rs",
    ] {
        let source = fs::read_to_string(workspace.join(relative)).expect("必须能够读取 API 源码");
        assert!(
            source.contains("ValidatedPageQuery::from_optional("),
            "{relative} 必须在 API 边界校验分页参数"
        );
    }

    let paginator = fs::read_to_string(workspace.join("crates/ryframe-db/src/pagination.rs"))
        .expect("必须能够读取统一分页器");
    assert!(paginator.contains("query: &ValidatedPageQuery"));
}

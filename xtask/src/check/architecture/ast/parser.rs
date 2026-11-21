use std::{fs, path::Path};

use tree_sitter::{Node, Parser};

use super::{AstResponse, FunctionRecord, UnsafeSyntaxRecord};

pub(super) fn parse_files(root: &Path, sources: &[String], templates: &[String]) -> AstResponse {
    let mut response = AstResponse::default();
    let mut parser = Parser::new();
    if let Err(error) = parser.set_language(&tree_sitter_rust::LANGUAGE.into()) {
        response
            .errors
            .push(format!("Rust AST grammar 加载失败：{error}"));
        return response;
    }
    let root = match root.canonicalize() {
        Ok(root) => root,
        Err(error) => {
            response
                .errors
                .push(format!("Rust AST 根目录无效：{error}"));
            return response;
        }
    };
    for (paths, template) in [(sources, false), (templates, true)] {
        for relative in paths {
            let path = match root.join(relative).canonicalize() {
                Ok(path) if path.starts_with(&root) && path.is_file() => path,
                Ok(_) => {
                    response
                        .errors
                        .push(format!("Rust AST 路径越出仓库或源码不存在：{relative}"));
                    continue;
                }
                Err(error) => {
                    response
                        .errors
                        .push(format!("Rust AST 源码不存在：{relative}：{error}"));
                    continue;
                }
            };
            match fs::read(&path) {
                Ok(source) => {
                    let relative = path
                        .strip_prefix(&root)
                        .expect("已验证仓库范围")
                        .to_string_lossy()
                        .replace('\\', "/");
                    parse_source(&mut parser, &relative, &source, template, &mut response);
                }
                Err(error) => response
                    .errors
                    .push(format!("无法读取 Rust 源码 {relative}：{error}")),
            }
        }
    }
    response
}

fn parse_source(
    parser: &mut Parser,
    path: &str,
    source: &[u8],
    template: bool,
    response: &mut AstResponse,
) {
    let Ok(source_text) = std::str::from_utf8(source) else {
        response
            .errors
            .push(format!("Rust AST 源码不是有效 UTF-8：{path}"));
        return;
    };
    let Some(tree) = parser.parse(source, None) else {
        response
            .errors
            .push(format!("tree-sitter 无法完整解析 Rust 源码：{path}"));
        return;
    };
    if !template && tree.root_node().has_error() {
        response
            .errors
            .push(format!("tree-sitter 无法完整解析 Rust 源码：{path}"));
        return;
    }
    if !template {
        functions(path, source_text, tree.root_node(), &mut response.functions);
    }
    unsafe_tokens(path, tree.root_node(), &mut response.unsafe_syntax);
}

fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

fn functions(path: &str, source: &str, root: Node<'_>, records: &mut Vec<FunctionRecord>) {
    let mut stack = vec![(root, Vec::<String>::new())];
    while let Some((node, mut child_scopes)) = stack.pop() {
        match node.kind() {
            "mod_item" | "trait_item" => {
                if let Some(name) = node.child_by_field_name("name") {
                    child_scopes.push(text(name, source).into());
                }
            }
            "impl_item" => {
                if let Some(implemented) = node.child_by_field_name("type") {
                    let label = match node.child_by_field_name("trait") {
                        Some(trait_name) => format!(
                            "{} for {}",
                            text(trait_name, source),
                            text(implemented, source)
                        ),
                        None => text(implemented, source).into(),
                    };
                    child_scopes.push(label);
                }
            }
            "function_item" if node.child_by_field_name("body").is_some() => {
                if let Some(name) = node.child_by_field_name("name") {
                    child_scopes.push(text(name, source).into());
                    let start_line = node.start_position().row + 1;
                    let end_line = node.end_position().row + 1;
                    let mut cursor = node.walk();
                    let private = !node
                        .named_children(&mut cursor)
                        .any(|child| child.kind() == "visibility_modifier");
                    records.push(FunctionRecord {
                        path: path.into(),
                        symbol: child_scopes.join("::"),
                        start_line,
                        end_line,
                        lines: end_line - start_line + 1,
                        private,
                    });
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        let children = node.named_children(&mut cursor).collect::<Vec<_>>();
        stack.extend(
            children
                .into_iter()
                .rev()
                .map(|child| (child, child_scopes.clone())),
        );
    }
}

fn unsafe_tokens(path: &str, root: Node<'_>, records: &mut Vec<UnsafeSyntaxRecord>) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "unsafe" {
            records.push(UnsafeSyntaxRecord {
                path: path.into(),
                line: node.start_position().row + 1,
                context: node
                    .parent()
                    .map_or("unknown", |parent| parent.kind())
                    .into(),
            });
        }
        let mut cursor = node.walk();
        let children = node.children(&mut cursor).collect::<Vec<_>>();
        stack.extend(children.into_iter().rev());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str, template: bool) -> AstResponse {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        let mut response = AstResponse::default();
        parse_source(
            &mut parser,
            "src/lib.rs",
            source.as_bytes(),
            template,
            &mut response,
        );
        response
    }

    #[test]
    fn concrete_functions_preserve_scopes_visibility_and_inclusive_lines() {
        let response = parse(
            concat!(
                "// fn fake() {}\n",
                "mod nested {\n",
                "pub trait Port { fn declaration(&self); fn default() {} }\n",
                "impl Port for Service {\n",
                "    pub(crate) fn execute(&self) {\n",
                "        let text = \"fn fake_in_string() {}\";\n",
                "        fn inner() {}\n",
                "    }\n",
                "}\n",
                "fn helper() {}\n",
                "}\n",
            ),
            false,
        );
        assert!(response.errors.is_empty());
        let functions = response
            .functions
            .iter()
            .map(|record| {
                (
                    record.symbol.as_str(),
                    record.private,
                    record.start_line,
                    record.end_line,
                    record.lines,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            functions,
            [
                ("nested::Port::default", true, 3, 3, 1),
                ("nested::Port for Service::execute", false, 5, 8, 4),
                ("nested::Port for Service::execute::inner", true, 7, 7, 1),
                ("nested::helper", true, 10, 10, 1),
            ]
        );
    }

    #[test]
    fn unsafe_tokens_include_macros_and_inactive_platform_code_but_ignore_text() {
        let response = parse(
            concat!(
                "// unsafe { ignored(); }\n",
                "const MESSAGE: &str = \"unsafe\";\n",
                "fn safe() { let r#unsafe = MESSAGE; }\n",
                "unsafe fn foreign() { unsafe { call(); } }\n",
                "unsafe impl Send for Value {}\n",
                "macro_rules! generated { () => { unsafe { call(); } } }\n",
                "#[cfg(target_os = \"nonexistent\")] unsafe fn platform() {}\n",
            ),
            false,
        );
        assert!(response.errors.is_empty());
        assert_eq!(
            response
                .unsafe_syntax
                .iter()
                .map(|record| record.line)
                .collect::<Vec<_>>(),
            [4, 4, 5, 6, 7]
        );
        assert_eq!(response.unsafe_syntax[0].context, "function_modifiers");
        assert_eq!(response.unsafe_syntax[1].context, "unsafe_block");
        assert_eq!(response.unsafe_syntax[2].context, "impl_item");
        let mut errors = Vec::new();
        assert_eq!(
            super::super::validate_no_unsafe(&response.unsafe_syntax, &mut errors),
            5
        );
        assert!(errors.iter().all(|error| error.contains("src/lib.rs")));
    }

    #[test]
    fn template_recovery_keeps_unsafe_while_source_errors_fail_closed() {
        let source = "pub fn {name}() { unsafe { call(); } }\n";
        let template = parse(source, true);
        assert!(template.errors.is_empty());
        assert!(template.functions.is_empty());
        assert_eq!(template.unsafe_syntax.len(), 1);
        assert_eq!(template.unsafe_syntax[0].line, 1);
        let ordinary = parse(source, false);
        assert_eq!(ordinary.errors.len(), 1);
        assert!(ordinary.functions.is_empty());
        assert!(ordinary.unsafe_syntax.is_empty());
    }

    #[test]
    fn invalid_utf8_in_comments_is_rejected_for_sources_and_templates() {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        for template in [false, true] {
            let mut response = AstResponse::default();
            parse_source(
                &mut parser,
                "src/lib.rs",
                b"// \xff\nfn valid() {}\n",
                template,
                &mut response,
            );
            assert_eq!(response.errors, ["Rust AST 源码不是有效 UTF-8：src/lib.rs"]);
            assert!(response.functions.is_empty());
            assert!(response.unsafe_syntax.is_empty());
        }
    }

    #[test]
    fn missing_and_escaping_sources_report_errors() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repo");
        fs::create_dir(&root).unwrap();
        fs::write(directory.path().join("outside.rs"), "fn outside() {}\n").unwrap();
        let response = parse_files(&root, &["missing.rs".into(), "../outside.rs".into()], &[]);
        assert_eq!(response.errors.len(), 2);
        assert!(response.functions.is_empty());
    }
}

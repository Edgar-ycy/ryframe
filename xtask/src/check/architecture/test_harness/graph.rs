use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use tree_sitter::{Node, Parser, Tree};

use super::attributes::{Attributes, Variant, text};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct Identity {
    pub(super) file: PathBuf,
    start: usize,
    end: usize,
}

pub(super) struct Case {
    pub(super) line: usize,
    pub(super) name: String,
    pub(super) opaque: bool,
}

type Cases = BTreeMap<Identity, Case>;

#[derive(Clone)]
struct Parsed {
    source: Arc<str>,
    tree: Tree,
}

pub(super) struct Scanner {
    root: PathBuf,
    parser: Parser,
    cache: BTreeMap<PathBuf, Parsed>,
}

struct Scope {
    file: PathBuf,
    directory: PathBuf,
    path_base: PathBuf,
    name: String,
}

struct Traversal {
    variant: Variant,
    stack: BTreeSet<PathBuf>,
    cases: Cases,
}

impl Scanner {
    pub(super) fn new(root: &Path) -> Result<Self, String> {
        let root = root
            .canonicalize()
            .map_err(|error| format!("无法解析仓库路径：{error}"))?;
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .map_err(|error| format!("无法加载 Rust tree-sitter：{error}"))?;
        Ok(Self {
            root,
            parser,
            cache: BTreeMap::new(),
        })
    }

    pub(super) fn collect(&mut self, entry: &Path, variant: Variant) -> Result<Cases, String> {
        let mut traversal = Traversal {
            variant,
            stack: BTreeSet::new(),
            cases: Cases::new(),
        };
        self.file(entry, String::new(), true, &mut traversal)?;
        Ok(traversal.cases)
    }

    fn parsed(&mut self, path: &Path) -> Result<Parsed, String> {
        if let Some(parsed) = self.cache.get(path) {
            return Ok(parsed.clone());
        }
        let source: Arc<str> = fs::read_to_string(path)
            .map_err(|error| format!("无法读取模块 {}：{error}", path.display()))?
            .into();
        let tree = self
            .parser
            .parse(source.as_ref(), None)
            .ok_or_else(|| format!("Rust AST 解析未完成：{}", path.display()))?;
        if tree.root_node().has_error() {
            return Err(format!("Rust AST 含语法错误，禁止跳过：{}", path.display()));
        }
        let parsed = Parsed { source, tree };
        self.cache.insert(path.to_path_buf(), parsed.clone());
        Ok(parsed)
    }

    fn file(
        &mut self,
        path: &Path,
        name: String,
        explicit_directory: bool,
        traversal: &mut Traversal,
    ) -> Result<(), String> {
        let file = checked_path(&self.root, path)?;
        if !traversal.stack.insert(file.clone()) {
            return Err(format!("模块存在递归文件环：{}", file.display()));
        }
        let parent = file.parent().ok_or("模块路径缺少父目录")?.to_path_buf();
        let directory =
            if explicit_directory || file.file_name().is_some_and(|name| name == "mod.rs") {
                parent.clone()
            } else {
                file.with_extension("")
            };
        let parsed = self.parsed(&file)?;
        let scope = Scope {
            file: file.clone(),
            directory,
            path_base: parent,
            name,
        };
        let result = self.items(parsed.tree.root_node(), &parsed.source, &scope, traversal);
        traversal.stack.remove(&file);
        result
    }

    fn items(
        &mut self,
        node: Node<'_>,
        source: &str,
        scope: &Scope,
        traversal: &mut Traversal,
    ) -> Result<(), String> {
        let mut cursor = node.walk();
        let children = node.named_children(&mut cursor).collect::<Vec<_>>();
        let inner = children
            .iter()
            .copied()
            .filter(|child| child.kind() == "inner_attribute_item")
            .collect::<Vec<_>>();
        let inner_attributes =
            Attributes::read(&inner, source, traversal.variant).map_err(|error| {
                let line = inner
                    .first()
                    .map_or(1, |node| node.start_position().row + 1);
                format!("{}:{line}：{error}", scope.file.display())
            })?;
        if inner_attributes.excluded {
            return Ok(());
        }
        if let Some(attribute) = inner_attributes.opaque {
            record(
                node,
                scope,
                format!("内部属性 #![{attribute}]"),
                true,
                traversal,
            );
        }
        let mut attributes = Vec::new();
        for child in children {
            match child.kind() {
                "attribute_item" => {
                    attributes.push(child);
                    continue;
                }
                "inner_attribute_item" | "line_comment" | "block_comment" => continue,
                _ => {}
            }
            let result = self.item(child, &attributes, source, scope, traversal);
            result.map_err(|error| {
                format!(
                    "{}:{}：{error}",
                    scope.file.display(),
                    child.start_position().row + 1
                )
            })?;
            attributes.clear();
        }
        Ok(())
    }

    fn item(
        &mut self,
        node: Node<'_>,
        attributes: &[Node<'_>],
        source: &str,
        scope: &Scope,
        traversal: &mut Traversal,
    ) -> Result<(), String> {
        let attributes = Attributes::read(attributes, source, traversal.variant)?;
        if attributes.excluded {
            return Ok(());
        }
        if let Some(attribute) = &attributes.opaque {
            record(node, scope, format!("属性 #[{attribute}]"), true, traversal);
        }
        match node.kind() {
            "mod_item" => self.module(node, &attributes, source, scope, traversal),
            "function_item" if attributes.test => {
                let name = node.child_by_field_name("name").ok_or("测试函数缺少名称")?;
                let name = joined(&scope.name, text(name, source)?);
                record(node, scope, name, false, traversal);
                Ok(())
            }
            "macro_invocation" | "macro_definition" => {
                record(
                    node,
                    scope,
                    text(node, source)?
                        .lines()
                        .next()
                        .unwrap_or("宏")
                        .to_owned(),
                    true,
                    traversal,
                );
                Ok(())
            }
            "expression_statement" => {
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    if child.kind() == "macro_invocation" {
                        self.item(child, &[], source, scope, traversal)?;
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn module(
        &mut self,
        node: Node<'_>,
        attributes: &Attributes,
        source: &str,
        scope: &Scope,
        traversal: &mut Traversal,
    ) -> Result<(), String> {
        let identifier = node.child_by_field_name("name").ok_or("模块缺少名称")?;
        let identifier = text(identifier, source)?.trim_start_matches("r#");
        let name = joined(&scope.name, identifier);
        if let Some(body) = node.child_by_field_name("body") {
            let directory = scope
                .directory
                .join(attributes.path.as_deref().unwrap_or(identifier));
            let child = Scope {
                file: scope.file.clone(),
                path_base: directory.clone(),
                directory,
                name,
            };
            return self.items(body, source, &child, traversal);
        }
        let path = if let Some(path) = &attributes.path {
            scope.path_base.join(path)
        } else {
            let candidates = [
                scope.directory.join(format!("{identifier}.rs")),
                scope.directory.join(identifier).join("mod.rs"),
            ];
            let existing = candidates
                .iter()
                .filter(|path| path.is_file())
                .collect::<Vec<_>>();
            if existing.len() != 1 {
                return Err(format!("模块 {name} 文件缺失或有歧义：{candidates:?}"));
            }
            existing[0].to_path_buf()
        };
        self.file(&path, name, attributes.path.is_some(), traversal)
    }
}

fn record(node: Node<'_>, scope: &Scope, name: String, opaque: bool, traversal: &mut Traversal) {
    let identity = Identity {
        file: scope.file.clone(),
        start: node.start_byte(),
        end: node.end_byte(),
    };
    traversal.cases.entry(identity).or_insert(Case {
        line: node.start_position().row + 1,
        name,
        opaque,
    });
}

fn joined(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}::{name}")
    }
}

pub(super) fn checked_path(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("无法解析仓库目录：{error}"))?;
    let path = path
        .canonicalize()
        .map_err(|error| format!("无法解析模块路径 {}：{error}", path.display()))?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err(format!("模块必须是仓库内的文件：{}", path.display()));
    }
    Ok(path)
}

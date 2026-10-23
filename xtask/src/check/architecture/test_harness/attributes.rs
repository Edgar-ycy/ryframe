use tree_sitter::Node;

#[derive(Clone, Copy)]
pub(super) struct Variant {
    pub(super) platform: &'static str,
    pub(super) resource: bool,
}

#[derive(Debug)]
enum Meta {
    Atom(String),
    Value(String, String),
    List(String, Vec<Meta>),
}

#[derive(Default)]
pub(super) struct Attributes {
    pub(super) excluded: bool,
    pub(super) path: Option<String>,
    pub(super) test: bool,
    pub(super) opaque: Option<String>,
}

impl Attributes {
    pub(super) fn read(nodes: &[Node<'_>], source: &str, variant: Variant) -> Result<Self, String> {
        let mut result = Self::default();
        for node in nodes {
            let mut cursor = node.walk();
            let attribute = node
                .named_children(&mut cursor)
                .find(|child| child.kind() == "attribute")
                .ok_or("缺少 Rust attribute AST 节点")?;
            let name = attribute.named_child(0).ok_or("属性缺少名称")?;
            let name = text(name, source)?;
            if !matches!(name, "cfg" | "cfg_attr" | "path" | "test") && !name.ends_with("::test") {
                if !inert_attribute(name) {
                    result.opaque = Some(name.to_owned());
                }
                continue;
            }
            let mut tokens = Vec::new();
            tokenize(attribute, source, &mut tokens)?;
            let mut parser = MetaParser {
                tokens: &tokens,
                index: 0,
            };
            let meta = parser.parse()?;
            if parser.index != tokens.len() {
                return Err("属性含未解析的 token，无法证明测试可达性".into());
            }
            result.apply(&meta, variant, true)?;
        }
        Ok(result)
    }

    fn apply(&mut self, meta: &Meta, variant: Variant, active: bool) -> Result<(), String> {
        match meta {
            Meta::List(name, arguments) if name == "cfg" && arguments.len() == 1 => {
                let enabled = evaluate(&arguments[0], variant)?;
                self.excluded |= active && !enabled;
            }
            Meta::List(name, arguments) if name == "cfg_attr" && arguments.len() >= 2 => {
                let enabled = evaluate(&arguments[0], variant)?;
                for attribute in &arguments[1..] {
                    self.apply(attribute, variant, active && enabled)?;
                }
            }
            Meta::Value(name, path) if name == "path" => {
                if active && self.path.replace(path.clone()).is_some() {
                    return Err("同一模块存在多个生效的 path 属性".into());
                }
            }
            Meta::Atom(name) | Meta::List(name, _)
                if name == "test" || name.ends_with("::test") =>
            {
                self.test |= active;
            }
            Meta::Atom(name) | Meta::Value(name, _) | Meta::List(name, _)
                if matches!(name.as_str(), "cfg" | "cfg_attr" | "path") =>
            {
                return Err(format!("不支持的可达性属性形式：{name}"));
            }
            Meta::Atom(name) | Meta::Value(name, _) | Meta::List(name, _)
                if active && !inert_attribute(name) =>
            {
                self.opaque = Some(name.clone());
            }
            _ => {}
        }
        Ok(())
    }
}

fn inert_attribute(name: &str) -> bool {
    matches!(
        name,
        "allow"
            | "expect"
            | "deny"
            | "forbid"
            | "doc"
            | "inline"
            | "cold"
            | "must_use"
            | "repr"
            | "non_exhaustive"
            | "deprecated"
            | "no_mangle"
            | "export_name"
            | "link"
            | "link_name"
            | "link_section"
            | "used"
            | "track_caller"
            | "target_feature"
            | "ignore"
            | "should_panic"
    )
}

fn evaluate(meta: &Meta, variant: Variant) -> Result<bool, String> {
    match meta {
        Meta::Atom(name) => match name.as_str() {
            "test" => Ok(true),
            "windows" => Ok(variant.platform == "windows"),
            "unix" => Ok(variant.platform == "linux"),
            _ => Err(format!("未知 cfg 谓词：{name}，必须补充明确的变体语义")),
        },
        Meta::Value(name, value) => match (name.as_str(), value.as_str()) {
            ("feature", "resource") => Ok(variant.resource),
            ("target_os", "linux" | "windows") => Ok(variant.platform == value.as_str()),
            _ => Err(format!("未知 cfg 条件：{name} = {value:?}，禁止猜测或跳过")),
        },
        Meta::List(name, arguments) => {
            let values = arguments
                .iter()
                .map(|item| evaluate(item, variant))
                .collect::<Result<Vec<_>, _>>()?;
            match name.as_str() {
                "all" => Ok(values.iter().all(|value| *value)),
                "any" => Ok(values.iter().any(|value| *value)),
                "not" if values.len() == 1 => Ok(!values[0]),
                _ => Err(format!("未知或参数数量错误的 cfg 运算：{name}")),
            }
        }
    }
}

struct MetaParser<'a> {
    tokens: &'a [String],
    index: usize,
}

impl MetaParser<'_> {
    fn take(&mut self) -> Result<&str, String> {
        let token = self.tokens.get(self.index).ok_or("属性意外结束")?;
        self.index += 1;
        Ok(token)
    }

    fn parse(&mut self) -> Result<Meta, String> {
        let mut name = self.take()?.to_owned();
        if name.is_empty() || !name.chars().all(|ch| ch == '_' || ch.is_alphanumeric()) {
            return Err(format!("属性标识符无效：{name}"));
        }
        while self
            .tokens
            .get(self.index)
            .is_some_and(|token| token == "::")
        {
            self.index += 1;
            name.push_str("::");
            name.push_str(self.take()?);
        }
        match self.tokens.get(self.index).map(String::as_str) {
            Some("=") => {
                self.index += 1;
                let value = string_value(self.take()?)?;
                Ok(Meta::Value(name, value))
            }
            Some("(") => {
                self.index += 1;
                let mut arguments = Vec::new();
                while self
                    .tokens
                    .get(self.index)
                    .is_some_and(|token| token != ")")
                {
                    arguments.push(self.parse()?);
                    if self
                        .tokens
                        .get(self.index)
                        .is_some_and(|token| token == ",")
                    {
                        self.index += 1;
                    } else if self.tokens.get(self.index).is_none_or(|token| token != ")") {
                        return Err("属性参数之间缺少逗号".into());
                    }
                }
                if self.take()? != ")" {
                    return Err("属性缺少结束括号".into());
                }
                Ok(Meta::List(name, arguments))
            }
            _ => Ok(Meta::Atom(name)),
        }
    }
}

fn tokenize(node: Node<'_>, source: &str, tokens: &mut Vec<String>) -> Result<(), String> {
    if matches!(node.kind(), "line_comment" | "block_comment") {
        return Ok(());
    }
    if node.child_count() == 0 || matches!(node.kind(), "string_literal" | "raw_string_literal") {
        tokens.push(text(node, source)?.to_owned());
        return Ok(());
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        tokenize(child, source, tokens)?;
    }
    Ok(())
}

fn string_value(token: &str) -> Result<String, String> {
    if let Some(raw) = token.strip_prefix('r') {
        let hashes = raw.chars().take_while(|ch| *ch == '#').count();
        let delimiter = format!("\"{}", "#".repeat(hashes));
        let value = raw
            .get(hashes..)
            .and_then(|raw| raw.strip_prefix('"'))
            .and_then(|raw| raw.strip_suffix(&delimiter));
        return value
            .map(str::to_owned)
            .ok_or_else(|| "无效的属性原始字符串".into());
    }
    serde_json::from_str(token).map_err(|_| "属性字符串含不支持的转义，无法证明可达性".into())
}

pub(super) fn text<'a>(node: Node<'_>, source: &'a str) -> Result<&'a str, String> {
    source
        .get(node.byte_range())
        .ok_or_else(|| "AST 节点越出 UTF-8 源码边界".into())
}

//! 按现有导入模板协议读取完整部门路径，限制 ZIP 与 XML 的内存边界。
use std::{
    collections::BTreeSet,
    io::{Cursor, Read},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use quick_xml::{NsReader, events::Event, name::ResolveResult};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::document;
use crate::Result;

const NS: &[u8] = b"http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const SHEET: &str = "xl/worksheets/sheet2.xml";

pub(super) fn template(value: &Value) -> Result<Value> {
    document::exact(
        value,
        &["base64", "bytes", "media_type", "sha256"],
        "部门模板响应",
    )?;
    let size = value["bytes"]
        .as_u64()
        .filter(|size| *size > 0 && *size <= 512 * 1024)
        .ok_or("部门模板长度超限")?;
    let media = document::text(&value["media_type"], "部门模板媒体类型")?;
    let digest = document::text(&value["sha256"], "部门模板摘要")?;
    let encoded = document::text(&value["base64"], "部门模板编码")?;
    if !media
        .to_ascii_lowercase()
        .starts_with("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || encoded.len() > (512 * 1024_usize).div_ceil(3) * 4
    {
        return Err("部门模板响应字段或边界无效".into());
    }
    let content = STANDARD
        .decode(encoded)
        .map_err(|_| "部门模板 base64 无效")?;
    if content.len() as u64 != size || hex::encode(Sha256::digest(&content)) != digest {
        return Err("部门模板长度或摘要不符".into());
    }
    Ok(json!({"bytes":size,"sha256":digest,"paths":paths(&content)?}))
}

pub(super) fn paths(content: &[u8]) -> Result<Vec<String>> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(content)).map_err(|_| "部门模板不是有效工作簿")?;
    if archive.len() > 1000 {
        return Err("部门模板 ZIP 成员数量超限".into());
    }
    let mut names = BTreeSet::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        let name = std::str::from_utf8(file.name_raw()).map_err(|_| "部门模板成员名编码无效")?;
        total = total
            .checked_add(file.size())
            .ok_or("部门模板解压长度溢出")?;
        if name.is_empty()
            || name.starts_with('/')
            || name.contains('\\')
            || name.split('/').any(|part| part == "..")
            || !names.insert(name.to_owned())
            || file.encrypted()
            || file.is_symlink()
            || total > MAX_BYTES
        {
            return Err("部门模板 ZIP 结构或解压边界无效".into());
        }
    }
    let strings = if names.contains("xl/sharedStrings.xml") {
        xml(&member(&mut archive, "xl/sharedStrings.xml")?)?
            .children
            .iter()
            .map(Node::text)
            .collect::<Vec<_>>()
    } else {
        vec![]
    };
    let sheet = xml(&member(&mut archive, SHEET)?)?;
    let mut paths = Vec::new();
    collect_cells(&sheet, &strings, &mut paths)?;
    if paths.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err("部门模板可用路径重复或未按当前契约排序".into());
    }
    Ok(paths)
}

fn member(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Vec<u8>> {
    let mut content = Vec::new();
    archive
        .by_name(name)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut content)?;
    if content.len() as u64 > MAX_BYTES {
        return Err("部门模板解压长度超限".into());
    }
    Ok(content)
}

#[derive(Default)]
struct Node {
    name: String,
    sheet: bool,
    reference: String,
    kind: String,
    text_parts: Vec<String>,
    children: Vec<Node>,
}

impl Node {
    fn text(&self) -> String {
        let mut text = self.text_parts.first().cloned().unwrap_or_default();
        for (index, child) in self.children.iter().enumerate() {
            text.push_str(&child.text());
            if let Some(part) = self.text_parts.get(index + 1) {
                text.push_str(part);
            }
        }
        text
    }
}

fn node(element: &quick_xml::events::BytesStart<'_>, sheet: bool) -> Result<Node> {
    let mut result = Node {
        name: std::str::from_utf8(element.local_name().as_ref())?.to_owned(),
        sheet,
        text_parts: vec![String::new()],
        ..Node::default()
    };
    for attr in element.attributes() {
        let attr = attr?;
        match attr.key.as_ref() {
            b"r" => {
                result.reference = attr
                    .normalized_value(quick_xml::XmlVersion::Explicit1_0)?
                    .into_owned()
            }
            b"t" => {
                result.kind = attr
                    .normalized_value(quick_xml::XmlVersion::Explicit1_0)?
                    .into_owned()
            }
            _ => {}
        }
    }
    Ok(result)
}

fn append(stack: &mut [Node], text: &str) {
    if let Some(node) = stack.last_mut()
        && let Some(part) = node.text_parts.last_mut()
    {
        part.push_str(text);
    }
}

fn xml(content: &[u8]) -> Result<Node> {
    let mut reader = NsReader::from_reader(content);
    let mut stack = Vec::<Node>::new();
    let mut root = None;
    loop {
        let (namespace, event) = reader.read_resolved_event()?;
        if matches!(namespace, ResolveResult::Unknown(_)) {
            return Err("部门模板 XML 含未绑定命名空间".into());
        }
        let sheet =
            matches!(namespace, ResolveResult::Bound(namespace) if namespace.as_ref() == NS);
        match event {
            Event::Start(element) => {
                if stack.len() >= 128 {
                    return Err("部门模板 XML 嵌套超限".into());
                }
                stack.push(node(&element, sheet)?);
            }
            Event::Empty(element) => finish_node(&mut stack, &mut root, node(&element, sheet)?)?,
            Event::End(_) => {
                let value = stack.pop().ok_or("部门模板 XML 闭合无效")?;
                finish_node(&mut stack, &mut root, value)?;
            }
            Event::Text(text) => {
                let text = text.decode()?;
                if stack.is_empty() && !text.trim().is_empty() {
                    return Err("部门模板 XML 根节点之外存在文本".into());
                }
                append(&mut stack, &text);
            }
            Event::CData(text) => {
                if stack.is_empty() {
                    return Err("部门模板 XML 根节点之外存在数据".into());
                }
                append(&mut stack, &text.decode()?);
            }
            Event::GeneralRef(reference) => {
                if stack.is_empty() {
                    return Err("部门模板 XML 根节点之外存在引用".into());
                }
                let text = reference.decode()?;
                let decoded = quick_xml::escape::unescape(&format!("&{text};"))?.into_owned();
                append(&mut stack, &decoded);
            }
            Event::DocType(_) => return Err("部门模板不接受 DTD".into()),
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err("部门模板 XML 未完整闭合".into());
    }
    root.ok_or_else(|| "部门模板 XML 缺少根节点".into())
}

fn finish_node(stack: &mut [Node], root: &mut Option<Node>, node: Node) -> Result<()> {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(node);
        parent.text_parts.push(String::new());
    } else if root.replace(node).is_some() {
        return Err("部门模板 XML 存在多个根节点".into());
    }
    Ok(())
}

fn collect_cells(node: &Node, strings: &[String], paths: &mut Vec<String>) -> Result<()> {
    if node.sheet
        && node.name == "c"
        && let Some(row) = node.reference.strip_prefix('A')
        && !row.starts_with('0')
        && !row.is_empty()
        && row.bytes().all(|b| b.is_ascii_digit())
        && (row.len() > 1 || row != "1")
    {
        let value = node
            .children
            .iter()
            .find(|child| child.sheet && child.name == "v");
        let text = if node.kind == "s"
            && let Some(value) = value
        {
            let index = value
                .text()
                .trim()
                .parse::<usize>()
                .map_err(|_| "部门模板 shared string 索引无效")?;
            strings
                .get(index)
                .ok_or("部门模板 shared string 索引越界")?
                .clone()
        } else {
            node.text()
        };
        let text = text.trim();
        if !text.is_empty() {
            paths.push(text.to_owned());
        }
    }
    for child in &node.children {
        collect_cells(child, strings, paths)?;
    }
    Ok(())
}

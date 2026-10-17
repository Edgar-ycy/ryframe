use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use super::{EXPECTED_BINARIES, execution_error};
use crate::Result;

const HEADER_SIZE: u64 = 512;
const BLOCK_SIZE: u64 = 512;
const FORBIDDEN_RUNTIME_TOOLS: &[&str] = &[
    "bin/bash",
    "bin/sh",
    "usr/bin/apt",
    "usr/bin/apt-get",
    "usr/bin/curl",
    "usr/bin/dpkg",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Regular,
    Link,
    Other,
}

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    kind: Kind,
    mode: u32,
    data_offset: u64,
    size: u64,
}

pub(super) fn violations(path: &Path) -> Result<Vec<String>> {
    let mut file = File::open(path).map_err(|error| execution_error("打开镜像归档", error))?;
    let entries = parse_entries(&mut file)?;
    let members = entries
        .into_iter()
        .map(|entry| (entry.name.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    let actual = members
        .values()
        .filter_map(|entry| {
            entry
                .name
                .strip_prefix("usr/local/bin/")
                .filter(|name| !name.contains('/'))
                .map(|name| (name.to_owned(), entry.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let expected = EXPECTED_BINARIES
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let mut violations = Vec::new();
    if actual
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>()
        != expected
    {
        violations.push(format!(
            "生产镜像 /usr/local/bin 条目必须精确为 {:?}，当前为 {:?}",
            expected,
            actual.keys().collect::<Vec<_>>()
        ));
    }
    for name in EXPECTED_BINARIES
        .iter()
        .filter(|name| actual.contains_key(**name))
    {
        let entry = &actual[*name];
        if entry.kind != Kind::Regular || entry.mode & 0o111 == 0 {
            violations.push(format!("生产镜像 {name} 必须是非链接的可执行普通文件"));
        }
    }
    let runtime_tools = FORBIDDEN_RUNTIME_TOOLS
        .iter()
        .filter(|name| members.contains_key(**name))
        .copied()
        .collect::<Vec<_>>();
    if !runtime_tools.is_empty() {
        violations.push(format!(
            "生产镜像不得包含 shell、包管理器或 curl: {}",
            runtime_tools.join(", ")
        ));
    }
    let suspicious = members
        .keys()
        .filter(|name| is_suspicious_generated_tool(name))
        .cloned()
        .collect::<Vec<_>>();
    if !suspicious.is_empty() {
        violations.push(format!(
            "生产镜像包含 generator/reset 可疑路径: {}",
            suspicious.join(", ")
        ));
    }
    let endpoint_hits = scan_endpoints(&mut file, &actual)?;
    if !endpoint_hits.is_empty() {
        violations.push(format!(
            "生产镜像二进制仍包含在线生成器端点或权限: {}",
            endpoint_hits.join(", ")
        ));
    }
    Ok(violations)
}

fn parse_entries(file: &mut File) -> Result<Vec<Entry>> {
    let length = file.metadata()?.len();
    let mut position = 0_u64;
    let mut entries = Vec::new();
    let mut pending_name = None;
    let mut pending_pax_path = None;
    while position < length {
        file.seek(SeekFrom::Start(position))?;
        let mut header = [0_u8; HEADER_SIZE as usize];
        file.read_exact(&mut header)?;
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        validate_checksum(&header)?;
        let size = parse_number(&header[124..136])?;
        let data_offset = position + HEADER_SIZE;
        if data_offset.saturating_add(size) > length {
            return Err("镜像归档条目的数据越过文件边界".into());
        }
        let kind = header[156];
        if matches!(kind, b'x' | b'g' | b'L' | b'K') {
            let data = read_metadata(file, data_offset, size)?;
            match kind {
                b'L' => pending_name = Some(trim_nul(&data)?),
                b'K' => {}
                b'x' => {
                    if let Some(path) = parse_pax(&data, b"path")? {
                        pending_pax_path = Some(path);
                    }
                }
                _ => {}
            }
        } else {
            let mut name = parse_entry_name(&header)?;
            if let Some(long) = pending_name.take() {
                name = long;
            }
            if let Some(pax) = pending_pax_path.take() {
                name = pax;
            }
            entries.push(Entry {
                name: normalize_name(&name),
                kind: match kind {
                    b'0' | b'7' | 0 => Kind::Regular,
                    b'1' | b'2' => Kind::Link,
                    _ => Kind::Other,
                },
                mode: parse_number(&header[100..108])? as u32,
                data_offset,
                size,
            });
        }
        position = data_offset
            .checked_add(size.div_ceil(BLOCK_SIZE) * BLOCK_SIZE)
            .ok_or("镜像归档偏移溢出")?;
    }
    Ok(entries)
}

fn parse_entry_name(header: &[u8; HEADER_SIZE as usize]) -> Result<String> {
    let name = trim_nul(&header[..100])?;
    let prefix = trim_nul(&header[345..500])?;
    Ok(if prefix.is_empty() {
        name
    } else {
        format!("{prefix}/{name}")
    })
}

fn parse_pax(source: &[u8], key: &[u8]) -> Result<Option<String>> {
    let mut position = 0;
    while position < source.len() {
        let space = source[position..]
            .iter()
            .position(|byte| *byte == b' ')
            .ok_or("PAX 头缺少长度分隔符")?;
        let length = std::str::from_utf8(&source[position..position + space])?
            .parse::<usize>()
            .map_err(|_| "PAX 头长度无效")?;
        if length == 0 || position + length > source.len() {
            return Err("PAX 头越过数据边界".into());
        }
        let record = &source[position + space + 1..position + length];
        let record = record.strip_suffix(b"\n").unwrap_or(record);
        if let Some(separator) = record.iter().position(|byte| *byte == b'=') {
            let (name, value) = record.split_at(separator);
            if name == key {
                return Ok(Some(std::str::from_utf8(&value[1..])?.to_owned()));
            }
        }
        position += length;
    }
    Ok(None)
}

fn read_metadata(file: &mut File, offset: u64, size: u64) -> Result<Vec<u8>> {
    if size > 1024 * 1024 {
        return Err("镜像归档元数据条目异常过大".into());
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut data = vec![0_u8; size as usize];
    file.read_exact(&mut data)?;
    Ok(data)
}

fn validate_checksum(header: &[u8; HEADER_SIZE as usize]) -> Result<()> {
    let expected = parse_number(&header[148..156])?;
    let actual = header
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if (148..156).contains(&index) {
                u64::from(b' ')
            } else {
                u64::from(*byte)
            }
        })
        .sum::<u64>();
    if actual != expected {
        return Err("镜像归档头校验和无效".into());
    }
    Ok(())
}

fn parse_number(field: &[u8]) -> Result<u64> {
    if field.first().is_some_and(|byte| byte & 0x80 != 0) {
        let mut bytes = field.iter();
        let first = bytes.next().ok_or("镜像归档数值字段为空")? & 0x7f;
        return bytes.try_fold(u64::from(first), |value, byte| {
            value
                .checked_mul(256)
                .and_then(|value| value.checked_add(u64::from(*byte)))
                .ok_or_else(|| "镜像归档数值溢出".into())
        });
    }
    let text = std::str::from_utf8(field)?.trim_matches([' ', '\0']);
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(text, 8).map_err(|_| "镜像归档八进制数值无效".into())
}

fn trim_nul(field: &[u8]) -> Result<String> {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    Ok(std::str::from_utf8(&field[..end])?.to_owned())
}

fn normalize_name(name: &str) -> String {
    name.strip_prefix("./")
        .unwrap_or(name)
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_owned()
}

fn is_suspicious_generated_tool(name: &str) -> bool {
    let lowered = name.to_lowercase();
    let base = lowered.rsplit('/').next().unwrap_or_default();
    let in_product_tree = ["usr/local/bin/", "opt/ryframe/", "var/lib/ryframe/"]
        .iter()
        .any(|prefix| lowered.starts_with(prefix));
    (in_product_tree && (base.contains("generator") || base.contains("reset")))
        || (base.contains("ryframe") && (base.contains("generator") || base.contains("reset")))
}

fn scan_endpoints(file: &mut File, binaries: &BTreeMap<String, Entry>) -> Result<Vec<String>> {
    let mut hits = Vec::new();
    for name in EXPECTED_BINARIES
        .iter()
        .filter(|name| binaries.contains_key(**name))
    {
        let entry = &binaries[*name];
        file.seek(SeekFrom::Start(entry.data_offset))?;
        if contains_generator_endpoint(file.take(entry.size), entry.size)? {
            hits.push(format!("/usr/local/bin/{name}"));
        }
    }
    Ok(hits)
}

fn contains_generator_endpoint(mut source: impl Read, size: u64) -> Result<bool> {
    const ENDPOINT: &[u8] = b"/api/v1/tools/gen";
    const PERMISSION: &[u8] = b"tools:gen";
    let mut buffer = [0_u8; 1024 * 1024];
    let mut overlap = Vec::<u8>::new();
    let mut read_total = 0_u64;
    while read_total < size {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            return Err("镜像归档二进制内容提前结束".into());
        }
        read_total += read as u64;
        let mut data = std::mem::take(&mut overlap);
        data.extend_from_slice(&buffer[..read]);
        if data
            .windows(ENDPOINT.len())
            .any(|window| window == ENDPOINT)
        {
            return Ok(true);
        }
        for index in 0..=data.len().saturating_sub(PERMISSION.len()) {
            if data.get(index..index + PERMISSION.len()) != Some(PERMISSION) {
                continue;
            }
            match data.get(index + PERMISSION.len()) {
                Some(byte) if !byte.is_ascii_lowercase() => return Ok(true),
                Some(_) => {}
                None if read_total == size => return Ok(true),
                None => {}
            }
        }
        let keep = data.len().min(128);
        overlap = data[data.len() - keep..].to_vec();
    }
    Ok(false)
}

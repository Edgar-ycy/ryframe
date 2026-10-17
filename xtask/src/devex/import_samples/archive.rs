use std::{
    borrow::Cow,
    collections::HashSet,
    io::{Cursor, Read, Write},
};

use quick_xml::{
    Reader, Writer, XmlVersion,
    events::{BytesEnd, BytesStart, BytesText, Event, attributes::Attribute},
    name::QName,
};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

use crate::Result;

use super::model::{
    HEADERS, LARGE_PAYLOAD_BYTES, LARGE_PAYLOAD_NAME, MAX_IMPORT_BYTES, MAX_TEMPLATE_BYTES,
    TemplateArchive,
};

const MAX_ARCHIVE_ENTRIES: usize = 1000;
const WORKSHEET: &str = "xl/worksheets/sheet1.xml";
const DEPARTMENT_SHEET: &str = "xl/worksheets/sheet2.xml";
const SHARED_STRINGS: &str = "xl/sharedStrings.xml";

#[derive(Debug, Default)]
struct RawCell {
    reference: Option<String>,
    kind: Option<String>,
    value: String,
}

pub(crate) fn inspect_template(content: &[u8]) -> Result<TemplateArchive> {
    let mut archive = open_archive(content)?;
    let names = validate_entries(&mut archive)?;
    let strings = match names.iter().any(|name| name == SHARED_STRINGS) {
        true => parse_shared_strings(&read_member(&mut archive, SHARED_STRINGS)?)?,
        false => Vec::new(),
    };
    let sheet1 =
        read_member(&mut archive, WORKSHEET).map_err(|_| "模板缺少有效工作表".to_string())?;
    let department_sheet = read_member(&mut archive, DEPARTMENT_SHEET)
        .map_err(|_| "模板缺少有效工作表".to_string())?;
    let rows = parse_rows(&sheet1)?;
    let header = rows
        .first()
        .ok_or("模板缺少用户导入表头")?
        .iter()
        .map(|cell| resolve_cell(cell, &strings))
        .collect::<Result<Vec<_>>>()?;
    if header != HEADERS {
        return Err("模板表头与当前用户导入契约不一致".into());
    }
    let department = parse_rows(&department_sheet)?
        .iter()
        .flatten()
        .find(|cell| cell.reference.as_deref() == Some("A2"))
        .map(|cell| resolve_cell(cell, &strings))
        .transpose()?
        .unwrap_or_default()
        .trim()
        .to_owned();
    if department.is_empty() {
        return Err("当前模板缺少真实可用部门".into());
    }
    Ok(TemplateArchive {
        member_names: names,
        sheet1,
        department,
    })
}

pub(crate) fn render_workbook(
    content: &[u8],
    template: &TemplateArchive,
    rows: &[[String; 5]],
    large: bool,
) -> Result<Vec<u8>> {
    if rows.is_empty() || rows.len() > 20_000 || rows.iter().any(|row| row.len() != HEADERS.len()) {
        return Err("导入样本行结构无效".into());
    }
    if large
        && template
            .member_names
            .iter()
            .any(|name| name == LARGE_PAYLOAD_NAME)
    {
        return Err("模板占用了大文件样本保留成员".into());
    }
    let sheet = rewrite_sheet(&template.sheet1, rows)?;
    let mut archive = open_archive(content)?;
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let mut replaced = false;
    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        if file.name() == WORKSHEET {
            if replaced {
                return Err("模板包含重复工作表成员".into());
            }
            let mut options = SimpleFileOptions::default().compression_method(file.compression());
            if let Some(permissions) = file.unix_mode() {
                options = options.unix_permissions(permissions);
            }
            if let Some(modified) = file.last_modified() {
                options = options.last_modified_time(modified);
            }
            writer.start_file(WORKSHEET, options)?;
            writer.write_all(&sheet)?;
            replaced = true;
        } else {
            writer.raw_copy_file(file)?;
        }
    }
    if !replaced {
        return Err("模板缺少用户导入数据区".into());
    }
    if large {
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        writer.start_file(LARGE_PAYLOAD_NAME, options)?;
        let mut remaining = LARGE_PAYLOAD_BYTES;
        let chunk = [0u8; 64 * 1024];
        while remaining > 0 {
            let write = remaining.min(chunk.len());
            writer.write_all(&chunk[..write])?;
            remaining -= write;
        }
    }
    let content = writer.finish()?.into_inner();
    if content.len() > MAX_IMPORT_BYTES {
        return Err("生成的导入样本超过10MiB产品上限".into());
    }
    if large && content.len() <= 2 * 1024 * 1024 {
        return Err("大文件样本没有达到multipart验证规模".into());
    }
    Ok(content)
}

fn open_archive(content: &[u8]) -> Result<ZipArchive<Cursor<&[u8]>>> {
    ZipArchive::new(Cursor::new(content)).map_err(|_| "模板不是有效XLSX ZIP或结构超出限额".into())
}

fn validate_entries<R: Read + std::io::Seek>(archive: &mut ZipArchive<R>) -> Result<Vec<String>> {
    if archive.is_empty() || archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err("XLSX ZIP 结构或解压边界无效".into());
    }
    let mut names = HashSet::new();
    let mut ordered = Vec::with_capacity(archive.len());
    let mut total = 0u64;
    for index in 0..archive.len() {
        let file = archive
            .by_index(index)
            .map_err(|_| "XLSX ZIP 结构或解压边界无效".to_string())?;
        let name = std::str::from_utf8(file.name_raw())
            .map_err(|_| "XLSX ZIP 结构或解压边界无效".to_string())?
            .to_owned();
        if invalid_member_name(&name)
            || !names.insert(name.clone())
            || file.encrypted()
            || file.is_symlink()
            || !matches!(
                file.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            )
        {
            return Err("XLSX ZIP 结构或解压边界无效".into());
        }
        total = total
            .checked_add(file.size())
            .ok_or("XLSX ZIP 结构或解压边界无效")?;
        if total > MAX_TEMPLATE_BYTES {
            return Err("XLSX ZIP 结构或解压边界无效".into());
        }
        ordered.push(name);
    }
    Ok(ordered)
}

fn invalid_member_name(name: &str) -> bool {
    if name.is_empty() || name.starts_with('/') || name.contains('\\') || name.contains(':') {
        return true;
    }
    let without_trailing = name.strip_suffix('/').unwrap_or(name);
    without_trailing
        .split('/')
        .any(|part| part.is_empty() || matches!(part, "." | ".."))
}

fn read_member<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Result<Vec<u8>> {
    let mut file = archive.by_name(name)?;
    let limit = usize::try_from(MAX_TEMPLATE_BYTES).unwrap_or(usize::MAX);
    let mut content = Vec::with_capacity(usize::try_from(file.size()).unwrap_or(0).min(limit));
    file.by_ref()
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut content)?;
    if content.len() > limit {
        return Err("XLSX ZIP 结构或解压边界无效".into());
    }
    Ok(content)
}

fn parse_shared_strings(xml: &[u8]) -> Result<Vec<String>> {
    let mut reader = Reader::from_reader(xml);
    let mut values = Vec::new();
    let mut current = None::<String>;
    loop {
        match reader.read_event()? {
            Event::Start(element) if element.local_name().as_ref() == b"si" => {
                current = Some(String::new());
            }
            Event::End(element) if element.local_name().as_ref() == b"si" => {
                values.push(current.take().ok_or("XLSX shared string 结构无效")?);
            }
            Event::Text(text) if current.is_some() => {
                current
                    .as_mut()
                    .ok_or("XLSX shared string 结构无效")?
                    .push_str(&text.decode()?);
            }
            Event::GeneralRef(reference) if current.is_some() => {
                current
                    .as_mut()
                    .ok_or("XLSX shared string 结构无效")?
                    .push_str(&decode_reference(&reference.decode()?));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(values)
}

fn parse_rows(xml: &[u8]) -> Result<Vec<Vec<RawCell>>> {
    let mut reader = Reader::from_reader(xml);
    let mut rows = Vec::new();
    let mut in_sheet_data = false;
    let mut row = None::<Vec<RawCell>>;
    let mut cell = None::<RawCell>;
    let mut nested = 0usize;
    loop {
        match reader.read_event()? {
            Event::Start(element) if element.local_name().as_ref() == b"sheetData" => {
                in_sheet_data = true;
            }
            Event::End(element) if element.local_name().as_ref() == b"sheetData" => break,
            Event::Start(element) if in_sheet_data && element.local_name().as_ref() == b"row" => {
                if row.replace(Vec::new()).is_some() {
                    return Err("模板工作表行结构无效".into());
                }
            }
            Event::End(element) if in_sheet_data && element.local_name().as_ref() == b"row" => {
                rows.push(row.take().ok_or("模板工作表行结构无效")?);
            }
            Event::Start(element)
                if in_sheet_data && row.is_some() && element.local_name().as_ref() == b"c" =>
            {
                if cell.is_none() {
                    cell = Some(RawCell {
                        reference: attribute(&element, b"r")?,
                        kind: attribute(&element, b"t")?,
                        value: String::new(),
                    });
                    nested = 0;
                } else {
                    nested += 1;
                }
            }
            Event::Start(_) if cell.is_some() => nested += 1,
            Event::End(element) if cell.is_some() && element.local_name().as_ref() == b"c" => {
                if nested == 0 {
                    let finished = cell.take().ok_or("模板工作表单元格结构无效")?;
                    row.as_mut()
                        .ok_or("模板工作表单元格结构无效")?
                        .push(finished);
                } else {
                    nested -= 1;
                }
            }
            Event::End(_) if cell.is_some() => {
                nested = nested.saturating_sub(1);
            }
            Event::Text(text) if cell.is_some() => {
                cell.as_mut()
                    .ok_or("模板工作表单元格结构无效")?
                    .value
                    .push_str(&text.decode()?);
            }
            Event::GeneralRef(reference) if cell.is_some() => {
                cell.as_mut()
                    .ok_or("模板工作表单元格结构无效")?
                    .value
                    .push_str(&decode_reference(&reference.decode()?));
            }
            Event::Eof => return Err("模板缺少有效工作表".into()),
            _ => {}
        }
    }
    Ok(rows)
}

fn resolve_cell(cell: &RawCell, strings: &[String]) -> Result<String> {
    if cell.kind.as_deref() != Some("s") {
        return Ok(cell.value.clone());
    }
    let index = cell
        .value
        .trim()
        .parse::<usize>()
        .map_err(|_| "XLSX shared string 索引无效".to_string())?;
    strings
        .get(index)
        .cloned()
        .ok_or_else(|| "XLSX shared string 索引无效".into())
}

fn attribute(element: &BytesStart<'_>, name: &[u8]) -> Result<Option<String>> {
    for attribute in element.attributes() {
        let attribute = attribute?;
        if attribute.key.local_name().as_ref() == name {
            return Ok(Some(
                attribute
                    .normalized_value(XmlVersion::Explicit1_0)?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

fn decode_reference(name: &str) -> String {
    match name {
        "lt" => "<".into(),
        "gt" => ">".into(),
        "amp" => "&".into(),
        "apos" => "'".into(),
        "quot" => "\"".into(),
        other => format!("&{other};"),
    }
}

fn rewrite_sheet(xml: &[u8], rows: &[[String; 5]]) -> Result<Vec<u8>> {
    let mut reader = Reader::from_reader(xml);
    let mut writer = Writer::new(Vec::new());
    let mut in_sheet_data = false;
    let mut row_depth = 0usize;
    let mut wrote_header = false;
    let mut copy_row = false;
    let mut dimension_done = false;
    let mut sheet_data_done = false;
    let mut prefix = None::<String>;
    loop {
        let event = reader.read_event()?;
        match event {
            Event::Start(element) if element.local_name().as_ref() == b"dimension" => {
                if dimension_done {
                    return Err("模板包含重复维度声明".into());
                }
                dimension_done = true;
                writer.write_event(Event::Start(rewrite_dimension(&element, rows.len())?))?;
            }
            Event::Empty(element) if element.local_name().as_ref() == b"dimension" => {
                if dimension_done {
                    return Err("模板包含重复维度声明".into());
                }
                dimension_done = true;
                writer.write_event(Event::Empty(rewrite_dimension(&element, rows.len())?))?;
            }
            Event::Start(element) if element.local_name().as_ref() == b"sheetData" => {
                if in_sheet_data || sheet_data_done {
                    return Err("模板包含重复数据区".into());
                }
                in_sheet_data = true;
                prefix = Some(prefix_of(&element));
                writer.write_event(Event::Start(element))?;
            }
            Event::End(element)
                if in_sheet_data && element.local_name().as_ref() == b"sheetData" =>
            {
                if row_depth != 0 {
                    return Err("模板工作表行结构无效".into());
                }
                write_generated_rows(&mut writer, prefix.as_deref().unwrap_or_default(), rows)?;
                writer.write_event(Event::End(element))?;
                in_sheet_data = false;
                sheet_data_done = true;
            }
            Event::Start(element)
                if in_sheet_data && row_depth == 0 && element.local_name().as_ref() == b"row" =>
            {
                row_depth = 1;
                copy_row = !wrote_header;
                if copy_row {
                    wrote_header = true;
                    writer.write_event(Event::Start(element))?;
                }
            }
            Event::Start(element) if in_sheet_data && row_depth > 0 => {
                row_depth += 1;
                if copy_row {
                    writer.write_event(Event::Start(element))?;
                }
            }
            Event::End(element) if in_sheet_data && row_depth > 0 => {
                let row_end = row_depth == 1;
                row_depth -= 1;
                if copy_row {
                    writer.write_event(Event::End(element))?;
                }
                if row_end {
                    copy_row = false;
                }
            }
            Event::Eof => break,
            event if in_sheet_data && row_depth > 0 && copy_row => writer.write_event(event)?,
            _event if in_sheet_data && row_depth > 0 => {}
            event => writer.write_event(event)?,
        }
    }
    if !dimension_done {
        return Err("模板缺少用户导入维度声明".into());
    }
    if !sheet_data_done || in_sheet_data {
        return Err("模板缺少有效工作表数据区".into());
    }
    if !wrote_header {
        return Err("模板缺少用户导入表头".into());
    }
    Ok(writer.into_inner())
}

fn rewrite_dimension(element: &BytesStart<'_>, row_count: usize) -> Result<BytesStart<'static>> {
    let mut attributes = Vec::new();
    for attribute in element.attributes().with_checks(false) {
        let attribute = attribute?;
        attributes.push((
            attribute.key.as_ref().to_vec(),
            attribute.value.into_owned(),
        ));
    }
    let mut owned = element.to_owned();
    owned.clear_attributes();
    let mut found = false;
    for (key, value) in attributes {
        if key
            .rsplit(|byte| *byte == b':')
            .next()
            .is_some_and(|local| local == b"ref")
        {
            found = true;
            push_raw_attribute(
                &mut owned,
                &key,
                format!("A1:E{}", row_count + 1).as_bytes(),
            );
        } else {
            push_raw_attribute(&mut owned, &key, &value);
        }
    }
    if found {
        Ok(owned)
    } else {
        Err("模板缺少用户导入维度声明".into())
    }
}

fn push_raw_attribute(element: &mut BytesStart<'static>, key: &[u8], value: &[u8]) {
    element.push_attribute(Attribute {
        key: QName(key),
        value: Cow::Borrowed(value),
    });
}

fn prefix_of(element: &BytesStart<'_>) -> String {
    let name = element.name();
    let raw = name.as_ref();
    raw.iter()
        .rposition(|byte| *byte == b':')
        .and_then(|index| std::str::from_utf8(&raw[..=index]).ok())
        .unwrap_or_default()
        .to_owned()
}

fn write_generated_rows<W: Write>(
    writer: &mut Writer<W>,
    prefix: &str,
    rows: &[[String; 5]],
) -> Result<()> {
    for (row_index, values) in rows.iter().enumerate() {
        let number = row_index + 2;
        let mut row = BytesStart::new(format!("{prefix}row"));
        row.push_attribute(("r", number.to_string().as_str()));
        writer.write_event(Event::Start(row))?;
        for (column, value) in "ABCDE".chars().zip(values) {
            let reference = format!("{column}{number}");
            let mut cell = BytesStart::new(format!("{prefix}c"));
            cell.push_attribute(("r", reference.as_str()));
            cell.push_attribute(("t", "inlineStr"));
            writer.write_event(Event::Start(cell))?;
            writer.write_event(Event::Start(BytesStart::new(format!("{prefix}is"))))?;
            writer.write_event(Event::Start(BytesStart::new(format!("{prefix}t"))))?;
            if !value.is_empty() {
                writer.write_event(Event::Text(BytesText::new(value)))?;
            }
            writer.write_event(Event::End(BytesEnd::new(format!("{prefix}t"))))?;
            writer.write_event(Event::End(BytesEnd::new(format!("{prefix}is"))))?;
            writer.write_event(Event::End(BytesEnd::new(format!("{prefix}c"))))?;
        }
        writer.write_event(Event::End(BytesEnd::new(format!("{prefix}row"))))?;
    }
    Ok(())
}

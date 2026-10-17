use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use quick_xml::{Reader, events::Event};
use sha2::{Digest, Sha256};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

pub(crate) const NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

pub(crate) struct Fixture {
    root: PathBuf,
}

impl Fixture {
    pub(crate) fn new(label: &str) -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests");
        fs::create_dir_all(&parent).unwrap();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = parent.join(format!(
            "import-samples-{label}-{}-{id}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        Self { root }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests");
        assert_eq!(self.root.parent(), Some(parent.as_path()));
        if self.root.exists() {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
}

pub(crate) fn template(
    path: &Path,
    department: &str,
    headers: &[&str],
    extra: Option<(&str, &[u8])>,
) -> String {
    let rows = vec![
        headers.iter().map(ToString::to_string).collect(),
        vec![
            "旧示例".into(),
            "示例".into(),
            String::new(),
            String::new(),
            "错误部门".into(),
        ],
    ];
    let reference = vec![vec!["部门完整路径".into()], vec![department.into()]];
    let file = File::create(path).unwrap();
    let mut archive = ZipWriter::new(file);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    archive
        .start_file("xl/worksheets/sheet1.xml", deflated)
        .unwrap();
    archive.write_all(&worksheet(&rows)).unwrap();
    archive
        .start_file("xl/worksheets/sheet2.xml", deflated)
        .unwrap();
    archive.write_all(&worksheet(&reference)).unwrap();
    archive.start_file("unchanged.txt", deflated).unwrap();
    archive
        .write_all("必须保留模板其他内容".as_bytes())
        .unwrap();
    if let Some((name, content)) = extra {
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        archive.start_file(name, stored).unwrap();
        archive.write_all(content).unwrap();
    }
    archive.finish().unwrap();
    sha256(&fs::read(path).unwrap())
}

pub(crate) fn write_archive(path: &Path, entries: &[(&str, &[u8])]) {
    let file = File::create(path).unwrap();
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, content) in entries {
        archive.start_file(*name, options).unwrap();
        archive.write_all(content).unwrap();
    }
    archive.finish().unwrap();
}

pub(crate) fn worksheet(rows: &[Vec<String>]) -> Vec<u8> {
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let column = char::from_u32('A' as u32 + width.saturating_sub(1) as u32).unwrap_or('A');
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <worksheet xmlns=\"{NS}\"><dimension ref=\"A1:{column}{}\"/><sheetData>",
        rows.len().max(1)
    );
    for (row_index, row) in rows.iter().enumerate() {
        let number = row_index + 1;
        xml.push_str(&format!("<row r=\"{number}\">"));
        for (column_index, value) in row.iter().enumerate() {
            let name = char::from_u32('A' as u32 + column_index as u32).unwrap();
            xml.push_str(&format!(
                "<c r=\"{name}{number}\" t=\"inlineStr\"><is><t>{}</t></is></c>",
                escape(value)
            ));
        }
        xml.push_str("</row>");
    }
    xml.push_str("</sheetData></worksheet>");
    xml.into_bytes()
}

pub(crate) fn sparse_worksheet(rows: &[(usize, Vec<String>)]) -> Vec<u8> {
    let max_row = rows.iter().map(|(row, _)| *row).max().unwrap_or(1);
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <worksheet xmlns=\"{NS}\"><dimension ref=\"A1:E{max_row}\"/><sheetData>"
    );
    for (row, values) in rows {
        xml.push_str(&format!("<row r=\"{row}\">"));
        for (column_index, value) in values.iter().enumerate() {
            let name = char::from_u32('A' as u32 + column_index as u32).unwrap();
            xml.push_str(&format!(
                "<c r=\"{name}{row}\" t=\"inlineStr\"><is><t>{}</t></is></c>",
                escape(value)
            ));
        }
        xml.push_str("</row>");
    }
    xml.push_str("</sheetData></worksheet>");
    xml.into_bytes()
}

pub(crate) fn archive_member(path: &Path, name: &str) -> Vec<u8> {
    let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
    let mut file = archive.by_name(name).unwrap();
    let mut content = Vec::new();
    file.read_to_end(&mut content).unwrap();
    content
}

pub(crate) fn archive_entry(path: &Path, name: &str) -> (CompressionMethod, u64) {
    let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
    let file = archive.by_name(name).unwrap();
    (file.compression(), file.size())
}

pub(crate) fn worksheet_rows(path: &Path) -> Vec<Vec<String>> {
    rows_from_xml(&archive_member(path, "xl/worksheets/sheet1.xml"))
}

pub(crate) fn rows_from_xml(xml: &[u8]) -> Vec<Vec<String>> {
    let mut reader = Reader::from_reader(xml);
    let mut rows = Vec::new();
    let mut row = None::<Vec<String>>;
    let mut cell = None::<String>;
    let mut cell_depth = 0usize;
    loop {
        match reader.read_event().unwrap() {
            Event::Start(element) if element.local_name().as_ref() == b"row" => {
                row = Some(Vec::new());
            }
            Event::End(element) if element.local_name().as_ref() == b"row" => {
                rows.push(row.take().unwrap());
            }
            Event::Start(element) if row.is_some() && element.local_name().as_ref() == b"c" => {
                cell = Some(String::new());
                cell_depth = 0;
            }
            Event::Start(_) if cell.is_some() => cell_depth += 1,
            Event::End(element) if cell.is_some() && element.local_name().as_ref() == b"c" => {
                if cell_depth == 0 {
                    row.as_mut().unwrap().push(cell.take().unwrap());
                } else {
                    cell_depth -= 1;
                }
            }
            Event::End(_) if cell.is_some() => cell_depth = cell_depth.saturating_sub(1),
            Event::Text(text) if cell.is_some() => {
                cell.as_mut().unwrap().push_str(&text.decode().unwrap());
            }
            Event::GeneralRef(reference) if cell.is_some() => {
                cell.as_mut()
                    .unwrap()
                    .push_str(&format!("&{};", reference.decode().unwrap()));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    rows
}

pub(crate) fn sheet_dimension(path: &Path) -> String {
    dimension(&archive_member(path, "xl/worksheets/sheet1.xml"))
}

pub(crate) fn dimension(xml: &[u8]) -> String {
    let mut reader = Reader::from_reader(xml);
    loop {
        match reader.read_event().unwrap() {
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref() == b"dimension" =>
            {
                for attribute in element.attributes() {
                    let attribute = attribute.unwrap();
                    if attribute.key.local_name().as_ref() == b"ref" {
                        return attribute
                            .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                            .unwrap()
                            .into_owned();
                    }
                }
                panic!("dimension 缺少 ref");
            }
            Event::Eof => panic!("缺少 dimension"),
            _ => {}
        }
    }
}

pub(crate) fn sha256(content: &[u8]) -> String {
    Sha256::digest(content)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

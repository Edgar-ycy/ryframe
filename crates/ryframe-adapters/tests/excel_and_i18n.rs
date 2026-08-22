use std::collections::BTreeMap;

use calamine::{Reader, open_workbook_auto};
use ryframe_adapters::{
    excel::{IncrementalExcelWriter, XLSX_MAX_DATA_ROWS},
    i18n::LocalizerLoader,
};
use ryframe_kernel::{AppError, Locale, LocalizedText};
use serde::Serialize;
use sha2::{Digest, Sha256};

const HEADERS: &[(&str, &str)] = &[("id", "编号"), ("name", "名称")];

#[derive(Serialize)]
struct Row {
    id: u64,
    name: &'static str,
}

#[test]
fn embedded_resources_load_into_kernel_localizer() {
    let localizer = LocalizerLoader::embedded().expect("内嵌语言资源应有效");
    assert_eq!(
        localizer.translate(Locale::ZhCn, "common.success"),
        "操作成功"
    );
    assert_eq!(
        localizer.translate(Locale::EnUs, "common.success"),
        "Operation successful"
    );

    let text = LocalizedText::Key {
        key: "user.welcome".into(),
        args: BTreeMap::from([("name".into(), "Alice".into())]),
    };
    assert_eq!(localizer.render(&text, Locale::EnUs), "Welcome Alice");
}

#[test]
fn incremental_writer_consumes_batches_and_removes_artifact_on_drop() {
    let mut writer = IncrementalExcelWriter::new("测试", HEADERS).expect("创建写入器");
    let first = writer
        .append_rows([Row { id: 1, name: "甲" }])
        .expect("写入首批");
    let second = writer
        .append_rows([Row { id: 2, name: "乙" }])
        .expect("写入次批");

    assert_eq!(first.batch_rows, 1);
    assert_eq!(second.total_rows, 2);
    assert_eq!(second.total_input_bytes, 8);

    let artifact = writer.finish().expect("完成工作簿");
    assert_eq!(artifact.data_rows(), 2);
    assert_eq!(artifact.input_bytes(), 8);
    assert!(artifact.size() > 0);
    let artifact_path = artifact.path().to_path_buf();
    let expected_sha256 = hex::encode(Sha256::digest(
        std::fs::read(&artifact_path).expect("读取表格产物"),
    ));
    assert_eq!(artifact.sha256(), expected_sha256);
    let mut workbook = open_workbook_auto(&artifact_path).expect("打开工作簿");
    let range = workbook.worksheet_range("测试").expect("读取工作表");
    assert_eq!(range.height(), 3);
    assert_eq!(
        range.get_value((2, 1)).map(ToString::to_string),
        Some("乙".into())
    );

    drop(workbook);
    drop(artifact);
    assert!(!artifact_path.exists());
}

#[test]
fn incremental_writer_enforces_configured_and_xlsx_row_limits() {
    let mut writer =
        IncrementalExcelWriter::with_row_limit("测试", HEADERS, 1).expect("创建受限写入器");
    writer
        .append_rows([Row { id: 1, name: "甲" }])
        .expect("写入限制内数据");
    let error = writer
        .append_rows([Row { id: 2, name: "乙" }])
        .expect_err("超过行上限必须失败");
    assert!(matches!(
        error,
        AppError::ExportRowLimitExceeded {
            matched_rows: 2,
            limit: 1
        }
    ));
    assert!(
        IncrementalExcelWriter::with_row_limit("测试", HEADERS, XLSX_MAX_DATA_ROWS + 1).is_err()
    );
}

use std::{
    collections::HashSet,
    ffi::OsString,
    fs,
    io::Write,
    process::{Command, Stdio},
};

use serde_json::{Value, json};
use zip::CompressionMethod;

use super::{
    devex::import_samples::{
        HEADERS, LARGE_PAYLOAD_BYTES, LARGE_PAYLOAD_NAME, PerformanceRequest, execute, model,
        model_sha256, prepare_browser, prepare_performance, template_receipt,
    },
    import_samples_fixture::{
        Fixture, archive_entry, archive_member, sha256, sheet_dimension, template, worksheet_rows,
        write_archive,
    },
};

pub(super) fn request(
    fixture: &Fixture,
    namespace: &str,
    directory: &str,
    template_sha256: &str,
) -> PerformanceRequest {
    PerformanceRequest {
        template: fixture.path("template.xlsx"),
        template_sha256: template_sha256.into(),
        model_sha256: model_sha256().unwrap(),
        directory: fixture.path(directory),
        namespace: namespace.into(),
        concurrency: 2,
        cycles: 2,
    }
}

#[test]
fn performance_rows_are_valid_and_preserve_template_members() {
    let fixture = Fixture::new("performance");
    let source = fixture.path("template.xlsx");
    let checksum = template(
        &source,
        "测试总部/测试部门",
        &HEADERS,
        Some(("unchanged.bin", b"member bytes")),
    );
    let manifest =
        prepare_performance(&request(&fixture, &"a".repeat(20), "samples", &checksum)).unwrap();
    assert_eq!(manifest.model, model());
    assert_eq!(manifest.files.len(), 4);
    let sheet2 = archive_member(&source, "xl/worksheets/sheet2.xml");
    for entry in &manifest.files {
        let path = fixture.path("samples").join(&entry.filename);
        assert_eq!(sha256(&fs::read(&path).unwrap()), entry.sha256);
        assert_eq!(
            worksheet_rows(&path)[1],
            [
                entry.username.clone(),
                "性能导入".into(),
                format!("{}@example.test", entry.username),
                String::new(),
                "测试总部/测试部门".into(),
            ]
        );
        assert_eq!(archive_member(&path, "xl/worksheets/sheet2.xml"), sheet2);
        assert_eq!(archive_member(&path, "unchanged.bin"), b"member bytes");
        assert_eq!(sheet_dimension(&path), "A1:E2");
    }
    assert_eq!(sha256(&fs::read(&source).unwrap()), checksum);
}

#[test]
fn rust_model_hash_matches_the_node_runtime_contract() {
    assert_eq!(
        model_sha256().unwrap(),
        "6d297f248e6b750425af25a95cafd035803fcbda03c3d8b5dd3503cc5b038c8f"
    );
}

#[test]
fn abba_samples_never_reuse_usernames_and_manifests_are_reproducible() {
    let fixture = Fixture::new("abba");
    let source = fixture.path("template.xlsx");
    let checksum = template(&source, "测试部门", &HEADERS, None);
    let mut used = HashSet::new();
    for (index, arm) in ["A", "B", "B", "A", "A", "B", "B", "A", "A", "B"]
        .into_iter()
        .enumerate()
    {
        let namespace = format!("{index:020x}");
        let request = request(&fixture, &namespace, &format!("{arm}-{index}"), &checksum);
        let manifest = prepare_performance(&request).unwrap();
        for file in &manifest.files {
            assert!(used.insert(file.username.clone()), "ABBA sample 用户名重复");
        }
    }
    assert_eq!(used.len(), 40);

    let first_request = request(&fixture, &"b".repeat(20), "first", &checksum);
    let mut second_request = first_request.clone();
    second_request.directory = fixture.path("second");
    let first = prepare_performance(&first_request).unwrap();
    let second = prepare_performance(&second_request).unwrap();
    assert_eq!(first, second);
}

#[test]
fn destination_sha_model_header_dimension_and_department_fail_before_output() {
    let fixture = Fixture::new("validation");
    let source = fixture.path("template.xlsx");
    let checksum = template(&source, "测试部门", &HEADERS, None);
    prepare_performance(&request(&fixture, &"c".repeat(20), "once", &checksum)).unwrap();
    assert!(prepare_performance(&request(&fixture, &"c".repeat(20), "once", &checksum)).is_err());

    let mut wrong_sha = request(&fixture, &"c".repeat(20), "wrong-sha", &checksum);
    wrong_sha.template_sha256 = "0".repeat(64);
    assert!(prepare_performance(&wrong_sha).is_err());
    let mut wrong_model = request(&fixture, &"c".repeat(20), "wrong-model", &checksum);
    wrong_model.model_sha256 = "0".repeat(64);
    assert!(prepare_performance(&wrong_model).is_err());
    for values in [
        (0, 1, "d".repeat(20)),
        (1, 0, "d".repeat(20)),
        (100, 101, "d".repeat(20)),
        (1, 1, "../../escape".into()),
    ] {
        let mut request = request(&fixture, &values.2, "dimension", &checksum);
        request.concurrency = values.0;
        request.cycles = values.1;
        assert!(prepare_performance(&request).is_err());
    }

    let header = fixture.path("header.xlsx");
    let header_sha = template(&header, "部门", &["旧结构"], None);
    let mut request = self::request(&fixture, &"e".repeat(20), "header-output", &header_sha);
    request.template = header;
    assert!(prepare_performance(&request).is_err());

    let department = fixture.path("department.xlsx");
    let department_sha = template(&department, "", &HEADERS, None);
    let mut request = self::request(
        &fixture,
        &"e".repeat(20),
        "department-output",
        &department_sha,
    );
    request.template = department;
    assert!(prepare_performance(&request).is_err());
    assert!(!fixture.path("header-output").exists());
    assert!(!fixture.path("department-output").exists());
}

#[test]
fn browser_fixture_has_success_duplicate_and_invalid_department_rows() {
    let fixture = Fixture::new("browser");
    let source = fixture.path("template.xlsx");
    let checksum = template(&source, "测试总部/测试部门", &HEADERS, None);

    let output = fixture.path("browser.xlsx");
    let receipt = prepare_browser(&source, &checksum, &output, "import-1234abcd", false).unwrap();
    assert_eq!(receipt.rows, 3);
    assert!(!receipt.large);
    assert_eq!(
        receipt.published.sha256,
        sha256(&fs::read(&output).unwrap())
    );
    assert_eq!(sheet_dimension(&output), "A1:E4");
    let rows = worksheet_rows(&output);
    assert_eq!(rows[0], HEADERS);
    assert_eq!(rows[1][0], rows[2][0]);
    assert_eq!(rows[1][4], "测试总部/测试部门");
    assert_eq!(rows[3][4], "不存在的隔离部门");
    assert_eq!(
        archive_member(&output, "unchanged.txt"),
        "必须保留模板其他内容".as_bytes()
    );
    assert!(prepare_browser(&source, &checksum, &output, "import-1234abcd", false).is_err());
}

#[test]
fn large_browser_fixture_uses_stored_padding_and_reserved_member_fails() {
    let fixture = Fixture::new("large");
    let source = fixture.path("template.xlsx");
    let checksum = template(&source, "测试部门", &HEADERS, None);
    let output = fixture.path("large.xlsx");
    let receipt = prepare_browser(&source, &checksum, &output, "import-large001", true).unwrap();
    assert!(receipt.published.bytes > LARGE_PAYLOAD_BYTES);
    assert!(receipt.published.bytes < 10 * 1024 * 1024);
    let (compression, size) = archive_entry(&output, LARGE_PAYLOAD_NAME);
    assert_eq!(compression, CompressionMethod::Stored);
    assert_eq!(size, LARGE_PAYLOAD_BYTES as u64);

    let reserved = fixture.path("reserved.xlsx");
    let reserved_sha = template(
        &reserved,
        "测试部门",
        &HEADERS,
        Some((LARGE_PAYLOAD_NAME, b"occupied")),
    );
    let failed = fixture.path("reserved-output.xlsx");
    assert!(prepare_browser(&reserved, &reserved_sha, &failed, "import-1234abcd", true).is_err());
    assert!(!failed.exists());
}

#[test]
fn template_receipt_is_read_only_and_binds_department_digest() {
    let fixture = Fixture::new("template");
    let source = fixture.path("template.xlsx");
    let checksum = template(&source, "总公司/研发部", &HEADERS, None);
    let before = fs::read(&source).unwrap();
    let receipt = template_receipt(&source, &checksum).unwrap();
    assert_eq!(receipt.template_sha256, checksum);
    assert_eq!(receipt.department_path, "总公司/研发部");
    assert_eq!(
        receipt.department_sha256,
        sha256("总公司/研发部".as_bytes())
    );
    assert_eq!(fs::read(&source).unwrap(), before);
}

#[test]
fn shared_string_header_and_sparse_department_row_are_parsed() {
    let fixture = Fixture::new("shared-strings");
    let source = fixture.path("shared.xlsx");
    let strings = [
        "用户名",
        "昵称",
        "邮箱",
        "手机号",
        "部门完整路径",
        "共享总部/研发部",
    ];
    let shared = format!(
        "<sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">{}</sst>",
        strings
            .iter()
            .map(|value| format!("<si><t>{value}</t></si>"))
            .collect::<String>()
    );
    let header = (0..5)
        .map(|index| {
            let column = char::from_u32('A' as u32 + index).unwrap();
            format!("<c r=\"{column}1\" t=\"s\"><v>{index}</v></c>")
        })
        .collect::<String>();
    let sheet1 = format!(
        "<worksheet xmlns=\"{}\"><dimension ref=\"A1:E1\"/><sheetData>\
         <row r=\"1\">{header}</row></sheetData></worksheet>",
        super::import_samples_fixture::NS
    );
    let sheet2 = format!(
        "<worksheet xmlns=\"{}\"><dimension ref=\"A1:A2\"/><sheetData>\
         <row r=\"2\"><c r=\"A2\" t=\"s\"><v>5</v></c></row>\
         </sheetData></worksheet>",
        super::import_samples_fixture::NS
    );
    write_archive(
        &source,
        &[
            ("xl/sharedStrings.xml", shared.as_bytes()),
            ("xl/worksheets/sheet1.xml", sheet1.as_bytes()),
            ("xl/worksheets/sheet2.xml", sheet2.as_bytes()),
        ],
    );
    let receipt = template_receipt(&source, &sha256(&fs::read(&source).unwrap())).unwrap();
    assert_eq!(receipt.department_path, "共享总部/研发部");
}

#[test]
fn hidden_protocol_is_stable_and_does_not_expose_private_paths() {
    let fixture = Fixture::new("protocol");
    let source = fixture.path("template.xlsx");
    let checksum = template(&source, "测试部门", &HEADERS, None);
    let request = json!({
        "template": source,
        "template_sha256": checksum,
        "model_sha256": model_sha256().unwrap(),
        "directory": fixture.path("performance"),
        "namespace": "f".repeat(20),
        "concurrency": 2,
        "cycles": 2,
    });
    let (code, output) = execute(
        &[OsString::from("performance")],
        &serde_json::to_vec(&request).unwrap(),
    );
    assert_eq!(code, 0);
    let response: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(response["ok"], true);
    assert_eq!(response["manifest"]["files"].as_array().unwrap().len(), 4);

    let failed = json!({
        "template": fixture.path("private-template-that-does-not-exist.xlsx"),
        "template_sha256": "0".repeat(64),
        "model_sha256": model_sha256().unwrap(),
        "directory": fixture.path("private-output"),
        "namespace": "1".repeat(20),
        "concurrency": 1,
        "cycles": 1,
    });
    let (code, output) = execute(
        &[OsString::from("performance")],
        &serde_json::to_vec(&failed).unwrap(),
    );
    assert_eq!(code, 1);
    assert_eq!(
        output,
        r#"{"ok":false,"reason":"import_fixture_preparation_failed"}"#
    );
    assert!(!output.contains("private-template"));
    assert_eq!(execute(&[OsString::from("browser")], b"").0, 2);
    let oversized = vec![b' '; 32_769];
    assert_eq!(execute(&[OsString::from("performance")], &oversized).0, 1);
    assert_eq!(
        execute(&[OsString::from("template")], b"").1,
        r#"{"ok":false,"reason":"import_fixture_preparation_failed"}"#
    );
}

#[test]
fn performance_request_rejects_unknown_fields_and_non_integer_dimensions() {
    let fixture = Fixture::new("request-shape");
    let base = json!({
        "template": fixture.path("template.xlsx"),
        "template_sha256": "a".repeat(64),
        "model_sha256": model_sha256().unwrap(),
        "directory": fixture.path("output"),
        "namespace": "a".repeat(20),
        "concurrency": 1,
        "cycles": 1,
    });
    for change in [
        json!({"extra": true}),
        json!({"concurrency": true}),
        json!({"cycles": 1.5}),
    ] {
        let mut value = base.clone();
        value
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        assert_eq!(
            execute(
                &[OsString::from("performance")],
                &serde_json::to_vec(&value).unwrap()
            )
            .0,
            1
        );
    }
}

#[test]
fn hidden_entry_is_absent_from_public_help_and_fails_with_only_the_stable_identifier() {
    let help = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(!String::from_utf8_lossy(&help.stdout).contains("__import-samples"));
    let failed = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__import-samples")
        .output()
        .unwrap();
    assert_eq!(failed.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&failed.stdout).trim(),
        r#"{"ok":false,"reason":"import_fixture_preparation_failed"}"#
    );
    assert!(failed.stderr.is_empty());
}

#[test]
fn hidden_binary_performance_protocol_reads_stdin_and_writes_the_manifest() {
    let fixture = Fixture::new("binary-protocol");
    let source = fixture.path("template.xlsx");
    let checksum = template(&source, "测试部门", &HEADERS, None);
    let request = json!({
        "template": source,
        "template_sha256": checksum,
        "model_sha256": model_sha256().unwrap(),
        "directory": fixture.path("binary-performance"),
        "namespace": "8".repeat(20),
        "concurrency": 2,
        "cycles": 2,
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["__import-samples", "performance"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["ok"], true);
    assert_eq!(response["manifest"]["files"].as_array().unwrap().len(), 4);
    assert!(
        fixture
            .path("binary-performance")
            .join("1-1.xlsx")
            .is_file()
    );
}

#[test]
fn performance_paths_support_unicode_and_spaces() {
    let fixture = Fixture::new("unicode");
    let directory = fixture.path("中文 空格");
    fs::create_dir(&directory).unwrap();
    let source = directory.join("真实 模板.xlsx");
    let checksum = template(&source, "测试部门", &HEADERS, None);
    let output = directory.join("导入 样本");
    let mut request = request(&fixture, &"9".repeat(20), "中文 样本", &checksum);
    request.template = source.clone();
    prepare_performance(&request).unwrap();
    assert!(request.directory.join("0-0.xlsx").is_file());

    let browser = directory.join("导入 浏览器.xlsx");
    let receipt = prepare_browser(&source, &checksum, &browser, "import-1234abcd", false).unwrap();
    assert_eq!(receipt.published.path, browser.to_string_lossy());
    assert_eq!(output.file_name().unwrap(), "导入 样本");
}

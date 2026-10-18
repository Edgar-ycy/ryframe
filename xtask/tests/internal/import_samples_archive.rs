use std::{
    ffi::OsString,
    fs::{self, File},
    io::Write,
    path::Path,
    process::Command,
    sync::{Arc, Barrier},
    thread,
};

use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use super::{
    devex::import_samples::{
        HEADERS, MAX_TEMPLATE_BYTES, execute, inspect_template, prepare_browser,
        prepare_performance, read_template_file,
    },
    import_samples_fixture::{Fixture, sha256, template, worksheet_rows, write_archive},
};

fn checksum(path: &Path) -> String {
    sha256(&fs::read(path).unwrap())
}

#[test]
fn zip_traversal_backslash_absolute_and_duplicate_members_fail_closed() {
    let fixture = Fixture::new("zip-paths");
    for (index, name) in ["../outside", "..\\outside", "/absolute"]
        .into_iter()
        .enumerate()
    {
        let path = fixture.path(&format!("invalid-{index}.xlsx"));
        write_archive(&path, &[(name, b"data")]);
        let error = inspect_template(&fs::read(&path).unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("ZIP"));
    }

    let duplicate = fixture.path("duplicate.xlsx");
    let file = File::create(&duplicate).unwrap();
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    archive.start_file("same1", options).unwrap();
    archive.write_all(b"first").unwrap();
    archive.start_file("same2", options).unwrap();
    archive.write_all(b"second").unwrap();
    archive.finish().unwrap();
    let mut content = fs::read(&duplicate).unwrap();
    let mut index = 0;
    while index + 5 <= content.len() {
        if &content[index..index + 5] == b"same2" {
            content[index..index + 5].copy_from_slice(b"same1");
            index += 5;
        } else {
            index += 1;
        }
    }
    fs::write(&duplicate, content).unwrap();
    assert!(inspect_template(&fs::read(&duplicate).unwrap()).is_err());
}

#[test]
fn encrypted_zip_member_and_symlink_member_are_rejected() {
    let fixture = Fixture::new("zip-members");
    let source = fixture.path("template.xlsx");
    template(&source, "测试部门", &HEADERS, None);
    let encrypted = fixture.path("encrypted.xlsx");
    let mut content = fs::read(&source).unwrap();
    for signature in [b"PK\x03\x04".as_slice(), b"PK\x01\x02".as_slice()] {
        let offset = content
            .windows(signature.len())
            .position(|window| window == signature)
            .unwrap();
        let flags_offset = offset + if signature[2] == 3 { 6 } else { 8 };
        let flags = u16::from_le_bytes([content[flags_offset], content[flags_offset + 1]]) | 1;
        content[flags_offset..flags_offset + 2].copy_from_slice(&flags.to_le_bytes());
    }
    fs::write(&encrypted, content).unwrap();
    assert!(inspect_template(&fs::read(&encrypted).unwrap()).is_err());

    let linked = fixture.path("linked.xlsx");
    let file = File::create(&linked).unwrap();
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default().unix_permissions(0o120777);
    archive.start_file("linked", options).unwrap();
    archive.write_all(b"target").unwrap();
    archive.finish().unwrap();
    assert!(inspect_template(&fs::read(&linked).unwrap()).is_err());
}

#[test]
fn member_count_and_uncompressed_size_limits_are_enforced() {
    let fixture = Fixture::new("zip-limits");
    let too_many = fixture.path("too-many.xlsx");
    let file = File::create(&too_many).unwrap();
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    for index in 0..1001 {
        archive
            .start_file(format!("entries/{index}"), options)
            .unwrap();
    }
    archive.finish().unwrap();
    assert!(inspect_template(&fs::read(&too_many).unwrap()).is_err());

    let expanded = fixture.path("expanded.xlsx");
    let file = File::create(&expanded).unwrap();
    let mut archive = ZipWriter::new(file);
    archive
        .start_file(
            "expanded",
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )
        .unwrap();
    let block = vec![0u8; 1024 * 1024];
    for _ in 0..17 {
        archive.write_all(&block).unwrap();
    }
    archive.finish().unwrap();
    assert!(inspect_template(&fs::read(&expanded).unwrap()).is_err());

    let oversized = fixture.path("oversized.xlsx");
    let file = File::create(&oversized).unwrap();
    file.set_len(MAX_TEMPLATE_BYTES + 1).unwrap();
    assert!(read_template_file(&oversized, &"0".repeat(64)).is_err());
}

#[test]
fn generated_output_over_product_limit_fails_without_partial_file() {
    let fixture = Fixture::new("output-limit");
    let source = fixture.path("template.xlsx");
    let padding = vec![0u8; 10 * 1024 * 1024];
    let checksum = template(
        &source,
        "测试部门",
        &HEADERS,
        Some(("stored.bin", &padding)),
    );
    let output = fixture.path("oversized-output.xlsx");
    let error = prepare_browser(&source, &checksum, &output, "import-1234abcd", false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("产品上限"));
    assert!(!output.exists());
}

#[test]
fn create_new_publication_rejects_existing_output_and_cleans_temporary_file() {
    let fixture = Fixture::new("create-new");
    let source = fixture.path("template.xlsx");
    let checksum = template(&source, "测试部门", &HEADERS, None);
    let output = fixture.path("browser.xlsx");
    fs::write(&output, "existing").unwrap();
    assert!(prepare_browser(&source, &checksum, &output, "import-1234abcd", false).is_err());
    assert_eq!(fs::read_to_string(&output).unwrap(), "existing");
    assert!(
        fs::read_dir(fixture.root())
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
    );
}

#[test]
fn concurrent_writers_publish_exactly_one_complete_file() {
    let fixture = Fixture::new("concurrent");
    let source = fixture.path("template.xlsx");
    let checksum = template(&source, "测试部门", &HEADERS, None);
    let output = fixture.path("winner.xlsx");
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for username in ["import-11111111", "import-22222222"] {
        let source = source.clone();
        let checksum = checksum.clone();
        let output = output.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            prepare_browser(&source, &checksum, &output, username, false)
                .map_err(|error| error.to_string())
        }));
    }
    let results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    let rows = worksheet_rows(&output);
    assert!(rows[1][0].starts_with("import-"));
    assert!(
        fs::read_dir(fixture.root())
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
    );
}

#[test]
fn parent_aliases_are_rejected_for_template_output_and_performance_directory() {
    let fixture = Fixture::new("aliases");
    let actual = fixture.path("真实父目录");
    fs::create_dir(&actual).unwrap();
    let source = actual.join("template.xlsx");
    let checksum = template(&source, "测试部门", &HEADERS, None);
    let alias = fixture.path("父目录链接");
    create_directory_alias(&alias, &actual);

    assert!(
        prepare_browser(
            &alias.join("template.xlsx"),
            &checksum,
            &fixture.path("linked-template-output.xlsx"),
            "import-1234abcd",
            false,
        )
        .is_err()
    );
    assert!(
        prepare_browser(
            &source,
            &checksum,
            &alias.join("linked-output.xlsx"),
            "import-1234abcd",
            false,
        )
        .is_err()
    );
    let request =
        super::import_samples_tests::request(&fixture, &"7".repeat(20), "alias-output", &checksum);
    let mut request = request;
    request.directory = alias.join("performance-output");
    assert!(prepare_performance(&request).is_err());
    assert!(fs::read_dir(&actual).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("output")
    }));
    remove_directory_alias(&alias);
}

#[test]
fn protocol_browser_errors_hide_template_and_output_paths() {
    let fixture = Fixture::new("protocol-redaction");
    let private = fixture.path("private-template-name.xlsx");
    let output = fixture.path("private-output-name.xlsx");
    let arguments = [
        OsString::from("browser"),
        OsString::from("--template"),
        private.into_os_string(),
        OsString::from("--template-sha256"),
        OsString::from("0".repeat(64)),
        OsString::from("--output"),
        output.clone().into_os_string(),
        OsString::from("--username"),
        OsString::from("import-1234abcd"),
    ];
    let (code, response) = execute(&arguments, b"");
    assert_eq!(code, 1);
    assert!(!response.contains("private-template"));
    assert!(!response.contains("private-output"));
    assert!(!output.exists());
}

#[test]
fn long_windows_paths_use_the_same_exclusive_publication_contract() {
    if !cfg!(windows) {
        return;
    }
    let fixture = Fixture::new("long-path");
    let mut directory = fixture.root().to_path_buf();
    for part in ["长路径", &"x".repeat(120), &"y".repeat(120)] {
        directory.push(part);
        fs::create_dir(&directory).unwrap();
    }
    let source = directory.join("template.xlsx");
    let checksum = template(&source, "测试部门", &HEADERS, None);
    let output = directory.join("import-deep0001.xlsx");
    let receipt = prepare_browser(&source, &checksum, &output, "import-deep0001", false).unwrap();
    assert_eq!(receipt.published.path, output.to_string_lossy());
    assert_eq!(receipt.published.sha256, checksum_member(&output));
    fs::remove_dir_all(&directory).unwrap();
}

fn checksum_member(path: &Path) -> String {
    sha256(&fs::read(path).unwrap())
}

#[cfg(windows)]
fn create_directory_alias(alias: &Path, target: &Path) {
    let status = Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(alias)
        .arg(target)
        .status()
        .unwrap();
    assert!(status.success());
}

#[cfg(not(windows))]
fn create_directory_alias(alias: &Path, target: &Path) {
    std::os::unix::fs::symlink(target, alias).unwrap();
}

#[cfg(windows)]
fn remove_directory_alias(alias: &Path) {
    fs::remove_dir(alias).unwrap();
}

#[cfg(not(windows))]
fn remove_directory_alias(alias: &Path) {
    fs::remove_file(alias).unwrap();
}

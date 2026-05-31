use ryframe_common::utils::file_upload::*;

#[test]
fn test_validate_extension_valid() {
    let allowed = vec!["jpg".into(), "png".into(), "pdf".into()];
    assert!(validate_extension("photo.jpg", &allowed).is_ok());
    assert!(validate_extension("photo.PNG", &allowed).is_ok());
    assert!(validate_extension("doc.pdf", &allowed).is_ok());
}

#[test]
fn test_validate_extension_invalid() {
    let allowed = vec!["jpg".into(), "png".into()];
    assert!(validate_extension("malware.exe", &allowed).is_err());
    assert!(validate_extension("script.sh", &allowed).is_err());
}

#[test]
fn test_validate_extension_empty_allowed() {
    let allowed: Vec<String> = vec![];
    assert!(validate_extension("anything.exe", &allowed).is_ok());
}

#[test]
fn test_validate_extension_no_extension() {
    let allowed = vec!["jpg".into()];
    assert!(validate_extension("noextension", &allowed).is_err());
}

#[test]
fn test_generate_storage_filename() {
    let name = generate_storage_filename("photo.jpg");
    assert!(name.ends_with(".jpg"));
    assert!(name.len() > 4);

    let name2 = generate_storage_filename("photo");
    assert!(name2.contains('.'));
}

#[test]
fn test_generate_storage_filename_uniqueness() {
    let a = generate_storage_filename("test.png");
    let b = generate_storage_filename("test.png");
    assert_ne!(a, b);
}

#[test]
fn test_get_content_type() {
    assert_eq!(get_content_type("photo.jpg"), "image/jpeg");
    assert_eq!(get_content_type("photo.jpeg"), "image/jpeg");
    assert_eq!(get_content_type("photo.png"), "image/png");
    assert_eq!(get_content_type("photo.gif"), "image/gif");
    assert_eq!(get_content_type("photo.webp"), "image/webp");
    assert_eq!(get_content_type("doc.pdf"), "application/pdf");
    assert_eq!(get_content_type("data.txt"), "text/plain");
    assert_eq!(get_content_type("archive.zip"), "application/zip");
    assert_eq!(get_content_type("file.xyz"), "application/octet-stream");
}

#[test]
fn test_format_file_size() {
    assert_eq!(format_file_size(0), "0 B");
    assert_eq!(format_file_size(512), "512 B");
    assert_eq!(format_file_size(1024), "1.00 KB");
    assert_eq!(format_file_size(1536), "1.50 KB");
    assert_eq!(format_file_size(1048576), "1.00 MB");
    assert_eq!(format_file_size(1073741824), "1.00 GB");
}

#[test]
fn test_upload_config_default() {
    let config = UploadConfig::default();
    assert_eq!(config.upload_dir, "uploads");
    assert_eq!(config.max_file_size, 10 * 1024 * 1024);
    assert!(config.allowed_extensions.contains(&"jpg".to_string()));
    assert!(config.allowed_extensions.contains(&"pdf".to_string()));
    assert!(config.allowed_extensions.contains(&"zip".to_string()));
}

#[test]
fn test_get_upload_dir() {
    let dir = get_upload_dir("uploads");
    let dir_str = dir.to_string_lossy();
    assert!(dir_str.starts_with("uploads"));
    assert!(dir_str.contains('/'));
}

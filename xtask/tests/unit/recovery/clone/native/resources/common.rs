use super::*;

#[test]
fn high_resolution_identity_preserves_full_volume_and_rejects_oversized_inode() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".local-tests");
    fs::create_dir_all(&root).unwrap();
    let directory = tempfile::tempdir_in(root).unwrap();
    let kernel = file_id::get_high_res_file_id(directory.path()).unwrap();
    let file_id::FileId::HighRes {
        volume_serial_number,
        file_id,
    } = kernel
    else {
        panic!("高精度内核身份读取返回了非 HighRes 记录");
    };
    assert_eq!(
        device_inode_for(directory.path(), &fs::metadata(directory.path()).unwrap()).unwrap(),
        (volume_serial_number, u64::try_from(file_id).unwrap())
    );
    let volume = 6_355_276_320_057_460_466;
    assert_eq!(
        high_res_identity(file_id::FileId::HighRes {
            volume_serial_number: volume,
            file_id: u128::from(u64::MAX),
        })
        .unwrap(),
        (volume, u64::MAX)
    );
    assert!(
        high_res_identity(file_id::FileId::HighRes {
            volume_serial_number: volume,
            file_id: u128::from(u64::MAX) + 1,
        })
        .is_err()
    );
}

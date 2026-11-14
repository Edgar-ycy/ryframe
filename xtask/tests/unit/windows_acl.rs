use super::{Descriptor, FULL_CONTROL, descriptor};

const CURRENT: &str = "S-1-5-21-1-2-3-1001";

fn sid(authority: u64, values: &[u32]) -> Vec<u8> {
    let mut result = vec![1, u8::try_from(values.len()).unwrap()];
    result.extend_from_slice(&authority.to_be_bytes()[2..]);
    for value in values {
        result.extend_from_slice(&value.to_le_bytes());
    }
    result
}

fn fixture() -> Vec<u8> {
    let owner = sid(5, &[21, 1, 2, 3, 1001]);
    let mut result = vec![0; 20];
    result[0] = 1;
    result[2..4].copy_from_slice(&0x9004u16.to_le_bytes());
    result[4..8].copy_from_slice(&20u32.to_le_bytes());
    let dacl = 20 + owner.len();
    result[16..20].copy_from_slice(&(dacl as u32).to_le_bytes());
    result.extend_from_slice(&owner);
    let ace_length = 8 + owner.len();
    result.extend_from_slice(&[2, 0]);
    result.extend_from_slice(&((8 + ace_length) as u16).to_le_bytes());
    result.extend_from_slice(&1u16.to_le_bytes());
    result.extend_from_slice(&[0, 0, 0, 3]);
    result.extend_from_slice(&(ace_length as u16).to_le_bytes());
    result.extend_from_slice(&FULL_CONTROL.to_le_bytes());
    result.extend_from_slice(&owner);
    result
}

fn parsed(bytes: &[u8]) -> Result<Descriptor, &'static str> {
    descriptor::parse(bytes, CURRENT.to_owned())
}

#[test]
fn binary_acl_preserves_raw_mask_flags_sid_endianness_and_original_rules() {
    let mut bytes = fixture();
    let baseline = parsed(&bytes).unwrap();
    baseline.verify_mysql(true).unwrap();
    assert_eq!(baseline.owner_sid, CURRENT);
    assert_eq!(baseline.entries[0].sid, CURRENT);
    assert!(baseline.protected);
    let ace = 20 + 28 + 8;
    bytes[ace + 4..ace + 8].copy_from_slice(&0xfedc_ba99u32.to_le_bytes());
    bytes[ace + 1] = 0x33;
    let value = parsed(&bytes).unwrap();
    assert_eq!(value.entries[0].rights, 0xfedc_ba99);
    assert_eq!(value.entries[0].flags, 0x33);
    assert_eq!(value.monitoring_value()["entries"][0]["inherited"], true);
    assert!(value.verify_mysql(false).is_err());
    bytes[ace + 4..ace + 8].copy_from_slice(&FULL_CONTROL.to_le_bytes());
    let inherited = parsed(&bytes).unwrap();
    inherited.verify_mysql(false).unwrap();
    assert!(inherited.verify_mysql(true).is_err());
    bytes[ace + 1] = 0x07; // OI | CI | NO_PROPAGATE_INHERIT_ACE
    assert!(parsed(&bytes).unwrap().verify_mysql(true).is_err());
    bytes[ace + 1] = 3;
    bytes[2..4].copy_from_slice(&0x8004u16.to_le_bytes());
    parsed(&bytes).unwrap().verify_mysql(false).unwrap();
    assert!(parsed(&bytes).unwrap().verify_mysql(true).is_err());
}

#[test]
fn binary_acl_rejects_truncation_offsets_counts_lengths_null_empty_and_unknown_aces() {
    let baseline = fixture();
    for length in 0..baseline.len() {
        assert!(parsed(&baseline[..length]).is_err(), "截断位置 {length}");
    }
    let dacl = 48;
    let ace = dacl + 8;
    for (offset, replacement) in [
        (0, vec![2]),
        (2, 0x1004u16.to_le_bytes().to_vec()),
        (2, 0x9000u16.to_le_bytes().to_vec()),
        (4, 0u32.to_le_bytes().to_vec()),
        (4, u32::MAX.to_le_bytes().to_vec()),
        (4, 21u32.to_le_bytes().to_vec()),
        (16, 0u32.to_le_bytes().to_vec()),
        (16, 20u32.to_le_bytes().to_vec()),
        (dacl, vec![1]),
        (dacl + 2, 7u16.to_le_bytes().to_vec()),
        (dacl + 4, 0u16.to_le_bytes().to_vec()),
        (dacl + 4, u16::MAX.to_le_bytes().to_vec()),
        (ace, vec![9]),
        (ace + 2, 12u16.to_le_bytes().to_vec()),
        (ace + 2, u16::MAX.to_le_bytes().to_vec()),
        (ace + 8, vec![0]),
        (ace + 9, vec![16]),
        (ace + 9, vec![4]),
    ] {
        let mut bytes = baseline.clone();
        bytes[offset..offset + replacement.len()].copy_from_slice(&replacement);
        assert!(parsed(&bytes).is_err(), "畸形字段 {offset}");
        if offset >= 16 || replacement == 0x9000u16.to_le_bytes() {
            assert_eq!(descriptor::owner(&bytes).unwrap(), CURRENT);
        } else {
            assert!(descriptor::owner(&bytes).is_err());
        }
    }
    let mut owner_only = baseline[..48].to_vec();
    owner_only[2..4].copy_from_slice(&0x8000u16.to_le_bytes());
    owner_only[16..20].fill(0);
    assert_eq!(descriptor::owner(&owner_only).unwrap(), CURRENT);
    assert!(parsed(&owner_only).is_err());
    let mut denied = baseline;
    denied[ace] = 1;
    assert_eq!(parsed(&denied).unwrap().entries[0].kind, 1);
    assert!(parsed(&denied).unwrap().verify_mysql(false).is_err());
}

#[test]
fn sid_authority_is_big_endian_and_subauthorities_are_little_endian() {
    let mut bytes = fixture();
    bytes[22..28].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    bytes[28..32].copy_from_slice(&0x1234_5678u32.to_le_bytes());
    assert_eq!(
        parsed(&bytes).unwrap().owner_sid,
        "S-1-0x010203040506-305419896-1-2-3-1001"
    );
}

#[cfg(windows)]
#[path = "windows_acl_live.rs"]
mod live;

use super::{Descriptor, ERROR, Entry};

type Result<T> = std::result::Result<T, &'static str>;

struct Owner {
    sid: String,
    start: usize,
    length: usize,
    control: u16,
}

/// 收紧新目录前只核对属主，不要求原 DACL 已满足收紧后的规则。
pub(super) fn owner(bytes: &[u8]) -> Result<String> {
    Ok(parse_owner(bytes)?.sid)
}

/// 只接受可完整核验的自相对描述符；缺失、NULL 和空 DACL 分别拒绝。
pub(super) fn parse(bytes: &[u8], current_sid: String) -> Result<Descriptor> {
    let owner = parse_owner(bytes)?;
    if owner.control & 4 == 0 {
        return Err("Windows ACL 缺少 DACL");
    }
    let raw_dacl = dword(bytes, 16)?;
    if raw_dacl == 0 {
        return Err("Windows ACL 为不受限的 NULL DACL");
    }
    let dacl = offset(bytes, 16)?;
    let size_offset = dacl.checked_add(2).ok_or(ERROR)?;
    let acl_end = dacl
        .checked_add(usize::from(word(bytes, size_offset)?))
        .ok_or(ERROR)?;
    let owner_end = owner.start.checked_add(owner.length).ok_or(ERROR)?;
    if owner.start < acl_end && dacl < owner_end {
        return Err(ERROR);
    }
    let entries = acl(bytes.get(dacl..).ok_or(ERROR)?)?;
    if entries.is_empty() {
        return Err("Windows ACL 是空 DACL");
    }
    Ok(Descriptor {
        current_sid,
        owner_sid: owner.sid,
        protected: owner.control & 0x1000 != 0,
        entries,
    })
}

fn parse_owner(bytes: &[u8]) -> Result<Owner> {
    let header = bytes.get(..20).ok_or(ERROR)?;
    let control = word(header, 2)?;
    if header[0] != 1 || control & 0x8000 == 0 {
        return Err(ERROR);
    }
    let start = offset(bytes, 4)?;
    let (sid, length) = sid(bytes.get(start..).ok_or(ERROR)?)?;
    Ok(Owner {
        sid,
        start,
        length,
        control,
    })
}

fn offset(bytes: &[u8], at: usize) -> Result<usize> {
    let value = usize::try_from(dword(bytes, at)?).map_err(|_| ERROR)?;
    if value < 20 || !value.is_multiple_of(4) || value >= bytes.len() {
        return Err(ERROR);
    }
    Ok(value)
}

fn acl(bytes: &[u8]) -> Result<Vec<Entry>> {
    let header = bytes.get(..8).ok_or(ERROR)?;
    if !matches!(header[0], 2 | 4) || header[1] != 0 || word(header, 6)? != 0 {
        return Err(ERROR);
    }
    let size = usize::from(word(header, 2)?);
    let count = usize::from(word(header, 4)?);
    let bytes = bytes
        .get(..size)
        .filter(|_| size >= 8 && size.is_multiple_of(4))
        .ok_or(ERROR)?;
    if count > (size - 8) / 16 {
        return Err(ERROR);
    }
    let mut cursor = 8usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let rest = bytes.get(cursor..).ok_or(ERROR)?;
        let length = usize::from(word(rest, 2)?);
        if length < 16 || !length.is_multiple_of(4) {
            return Err(ERROR);
        }
        let ace = rest.get(..length).ok_or(ERROR)?;
        if !matches!(ace[0], 0 | 1) {
            return Err("Windows ACL 含无法核验的 ACE 类型");
        }
        let (sid, consumed) = sid(&ace[8..])?;
        if consumed.checked_add(8) != Some(length) {
            return Err(ERROR);
        }
        entries.push(Entry {
            sid,
            kind: ace[0],
            rights: dword(ace, 4)?,
            flags: ace[1],
        });
        cursor = cursor.checked_add(length).ok_or(ERROR)?;
    }
    // AclSize 可包含预留空间；有效 ACE 数量由已完整消费的 AceCount 决定。
    Ok(entries)
}

fn sid(bytes: &[u8]) -> Result<(String, usize)> {
    let header = bytes.get(..8).ok_or(ERROR)?;
    let count = usize::from(header[1]);
    if header[0] != 1 || count > 15 {
        return Err(ERROR);
    }
    let size = count
        .checked_mul(4)
        .and_then(|n| n.checked_add(8))
        .ok_or(ERROR)?;
    let bytes = bytes.get(..size).ok_or(ERROR)?;
    let authority = header[2..8]
        .iter()
        .fold(0u64, |value, byte| (value << 8) | u64::from(*byte));
    let mut value = if authority > u64::from(u32::MAX) {
        format!("S-1-0x{authority:012x}")
    } else {
        format!("S-1-{authority}")
    };
    for index in 0..count {
        value.push_str(&format!("-{}", dword(bytes, 8 + index * 4)?));
    }
    Ok((value, size))
}

fn word(bytes: &[u8], offset: usize) -> Result<u16> {
    let end = offset.checked_add(2).ok_or(ERROR)?;
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..end)
            .ok_or(ERROR)?
            .try_into()
            .map_err(|_| ERROR)?,
    ))
}

fn dword(bytes: &[u8], offset: usize) -> Result<u32> {
    let end = offset.checked_add(4).ok_or(ERROR)?;
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..end)
            .ok_or(ERROR)?
            .try_into()
            .map_err(|_| ERROR)?,
    ))
}

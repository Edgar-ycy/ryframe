use super::super::{StorageError, StorageResult, key_segments};
use super::STAGING_DIRECTORY_NAME;

pub(super) fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    #[cfg(not(windows))]
    {
        false
    }
}

fn is_windows_device_name(segment: &str) -> bool {
    let stem = segment
        .split('.')
        .next()
        .unwrap_or(segment)
        .trim_end_matches([' ', '.'])
        .to_ascii_lowercase();
    if matches!(
        stem.as_str(),
        "con" | "prn" | "aux" | "nul" | "clock$" | "conin$" | "conout$"
    ) {
        return true;
    }

    let numbered_device = |prefix: &str| {
        stem.strip_prefix(prefix).is_some_and(|number| {
            matches!(
                number,
                "1" | "2"
                    | "3"
                    | "4"
                    | "5"
                    | "6"
                    | "7"
                    | "8"
                    | "9"
                    | "\u{00b9}"
                    | "\u{00b2}"
                    | "\u{00b3}"
            )
        })
    };
    numbered_device("com") || numbered_device("lpt")
}

pub(super) fn local_key_segments(key: &str) -> StorageResult<Vec<&str>> {
    let segments = key_segments(key)?;
    for segment in &segments {
        if segment.eq_ignore_ascii_case(STAGING_DIRECTORY_NAME) {
            return Err(StorageError::InvalidLocation(
                "object key uses the reserved local-storage staging namespace".to_owned(),
            ));
        }
        if segment.contains(':')
            || segment.ends_with('.')
            || segment.ends_with(' ')
            || is_windows_device_name(segment)
        {
            return Err(StorageError::InvalidLocation(
                "object key contains a segment that is unsafe on Windows".to_owned(),
            ));
        }
    }
    Ok(segments)
}

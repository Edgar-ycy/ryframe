pub(super) fn normalize_check_clause(value: &str) -> String {
    // MySQL information_schema may render literal delimiters as
    // `_utf8mb4\'value\'`. Only normalize syntax outside literals. Literal
    // bytes (including case, whitespace, backticks and charset-like text) are
    // semantic and must survive the exact comparison unchanged.
    let bytes = value.as_bytes();
    let mut normalized = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if let Some(introducer_len) = charset_introducer_len(bytes, index) {
            index += introducer_len;
            continue;
        }
        if let Some((delimiter, consumed)) = quote_token_at(bytes, index) {
            normalized.push(delimiter);
            index += consumed;
            index =
                normalize_quoted_literal(bytes, index, delimiter, consumed == 2, &mut normalized);
            continue;
        }
        let byte = bytes[index];
        if !byte.is_ascii_whitespace() && byte != b'`' {
            normalized.push(byte.to_ascii_lowercase());
        }
        index += 1;
    }
    strip_redundant_outer_parentheses(
        String::from_utf8(normalized).expect("CHECK clause normalization preserves UTF-8"),
    )
}

fn charset_introducer_len(bytes: &[u8], index: usize) -> Option<usize> {
    [b"_utf8mb4".as_slice(), b"_ascii".as_slice()]
        .into_iter()
        .find(|introducer| {
            let end = index + introducer.len();
            bytes
                .get(index..end)
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(introducer))
                && quote_token_at(bytes, end).is_some()
        })
        .map(<[u8]>::len)
}

fn quote_token_at(bytes: &[u8], index: usize) -> Option<(u8, usize)> {
    match bytes.get(index).copied() {
        Some(delimiter @ (b'\'' | b'"')) => Some((delimiter, 1)),
        Some(b'\\') => bytes
            .get(index + 1)
            .copied()
            .filter(|byte| matches!(byte, b'\'' | b'"'))
            .map(|delimiter| (delimiter, 2)),
        _ => None,
    }
}

fn normalize_quoted_literal(
    bytes: &[u8],
    mut index: usize,
    delimiter: u8,
    escaped_delimiters: bool,
    output: &mut Vec<u8>,
) -> usize {
    while index < bytes.len() {
        if escaped_delimiters && bytes.get(index) == Some(&b'\\') {
            let slash_start = index;
            while bytes.get(index) == Some(&b'\\') {
                index += 1;
            }
            let slash_count = index - slash_start;
            if bytes.get(index) == Some(&delimiter) {
                if slash_count == 1 {
                    output.push(delimiter);
                    return index + 1;
                }
                if slash_count >= 3 && slash_count % 2 == 1 {
                    // MySQL may render an apostrophe inside an escaped-delimiter
                    // literal as three slashes plus the quote. Canonicalize it to
                    // SQL's doubled-quote representation while preserving any
                    // additional literal backslashes.
                    output.extend(std::iter::repeat_n(b'\\', (slash_count - 3) / 2));
                    output.extend_from_slice(&[delimiter, delimiter]);
                    index += 1;
                    continue;
                }
            }
            output.extend_from_slice(&bytes[slash_start..index]);
            continue;
        }
        if !escaped_delimiters
            && bytes.get(index) == Some(&b'\\')
            && bytes.get(index + 1) == Some(&delimiter)
        {
            output.extend_from_slice(&[delimiter, delimiter]);
            index += 2;
            continue;
        }
        if bytes[index] == delimiter {
            if !escaped_delimiters && bytes.get(index + 1) == Some(&delimiter) {
                output.extend_from_slice(&[delimiter, delimiter]);
                index += 2;
                continue;
            }
            output.push(delimiter);
            return index + 1;
        }
        output.push(bytes[index]);
        index += 1;
    }
    index
}

fn strip_redundant_outer_parentheses(mut value: String) -> String {
    while is_wrapped_by_single_outer_group(&value) {
        value = value[1..value.len() - 1].to_owned();
    }
    value
}

fn is_wrapped_by_single_outer_group(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'(' || bytes[bytes.len() - 1] != b')' {
        return false;
    }
    let mut depth = 0_i32;
    let mut quote = None;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(delimiter) = quote {
            if byte == b'\\' {
                index = (index + 2).min(bytes.len());
                continue;
            }
            if byte == delimiter {
                if bytes.get(index + 1) == Some(&delimiter) {
                    index += 2;
                    continue;
                }
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        } else if byte == b'(' {
            depth += 1;
        } else if byte == b')' {
            depth -= 1;
            if depth == 0 && index + 1 != bytes.len() {
                return false;
            }
            if depth < 0 {
                return false;
            }
        }
        index += 1;
    }
    depth == 0 && quote.is_none()
}

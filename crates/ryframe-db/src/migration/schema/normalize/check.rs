pub fn normalize_check_clause(value: &str) -> String {
    // MySQL information_schema 可能增加外层括号、反引号及字符集引介符。
    // 先按词法 token 解析，再用类型和长度编码，避免删除空白后把不同表达式拼成同一串。
    let mut tokens = check_tokens(value);
    strip_redundant_outer_parentheses(&mut tokens);
    strip_redundant_atomic_parentheses(&mut tokens);
    encode_check_tokens(&tokens)
}

#[derive(Debug, Eq, PartialEq)]
enum CheckToken {
    Identifier(String),
    Keyword(String),
    Number(String),
    Literal(String),
    Symbol(String),
    OpenParenthesis,
    CloseParenthesis,
}

fn check_tokens(value: &str) -> Vec<CheckToken> {
    let bytes = value.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if let Some(introducer_len) = charset_introducer_len(bytes, index) {
            index += introducer_len;
            continue;
        }
        if let Some((delimiter, consumed)) = quote_token_at(bytes, index) {
            let mut literal = vec![delimiter];
            index += consumed;
            index = normalize_quoted_literal(bytes, index, delimiter, consumed == 2, &mut literal);
            tokens.push(CheckToken::Literal(
                String::from_utf8(literal).expect("CHECK literal normalization preserves UTF-8"),
            ));
            continue;
        }
        if bytes[index] == b'`' {
            let (identifier, next) = quoted_identifier(bytes, index + 1);
            tokens.push(CheckToken::Identifier(identifier));
            index = next;
            continue;
        }
        if bytes[index].is_ascii_digit()
            || (bytes[index] == b'.' && bytes.get(index + 1).is_some_and(u8::is_ascii_digit))
        {
            let end = number_end(bytes, index);
            tokens.push(CheckToken::Number(lower_ascii(&bytes[index..end])));
            index = end;
            continue;
        }
        if is_word_byte(bytes[index]) {
            let end = word_end(bytes, index);
            let word = lower_ascii(&bytes[index..end]);
            if is_check_keyword(&word) {
                tokens.push(CheckToken::Keyword(word));
            } else {
                tokens.push(CheckToken::Identifier(word));
            }
            index = end;
            continue;
        }
        match bytes[index] {
            b'(' => tokens.push(CheckToken::OpenParenthesis),
            b')' => tokens.push(CheckToken::CloseParenthesis),
            _ => {
                let end = symbol_end(bytes, index);
                tokens.push(CheckToken::Symbol(lower_ascii(&bytes[index..end])));
                index = end;
                continue;
            }
        }
        index += 1;
    }
    tokens
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

fn quoted_identifier(bytes: &[u8], mut index: usize) -> (String, usize) {
    let mut identifier = Vec::new();
    while index < bytes.len() {
        if bytes[index] == b'`' {
            if bytes.get(index + 1) == Some(&b'`') {
                identifier.push(b'`');
                index += 2;
                continue;
            }
            return (
                String::from_utf8(identifier)
                    .expect("CHECK identifier normalization preserves UTF-8")
                    .to_ascii_lowercase(),
                index + 1,
            );
        }
        identifier.push(bytes[index]);
        index += 1;
    }
    (
        String::from_utf8(identifier)
            .expect("CHECK identifier normalization preserves UTF-8")
            .to_ascii_lowercase(),
        index,
    )
}

fn number_end(bytes: &[u8], start: usize) -> usize {
    if bytes.get(start..start + 2).is_some_and(|prefix| {
        prefix.eq_ignore_ascii_case(b"0x") || prefix.eq_ignore_ascii_case(b"0b")
    }) {
        return (start + 2..bytes.len())
            .find(|index| !bytes[*index].is_ascii_hexdigit())
            .unwrap_or(bytes.len());
    }
    let mut index = start;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
    }
    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        let exponent = index;
        index += 1;
        if matches!(bytes.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        let digits = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == digits {
            return exponent;
        }
    }
    index.max(start + 1)
}

fn word_end(bytes: &[u8], start: usize) -> usize {
    (start + 1..bytes.len())
        .find(|index| !is_word_byte(bytes[*index]))
        .unwrap_or(bytes.len())
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$') || !byte.is_ascii()
}

fn is_check_keyword(value: &str) -> bool {
    matches!(
        value,
        "and"
            | "as"
            | "between"
            | "binary"
            | "case"
            | "collate"
            | "div"
            | "else"
            | "end"
            | "escape"
            | "false"
            | "in"
            | "interval"
            | "is"
            | "like"
            | "member"
            | "mod"
            | "not"
            | "null"
            | "of"
            | "or"
            | "regexp"
            | "rlike"
            | "sounds"
            | "then"
            | "true"
            | "unknown"
            | "when"
            | "xor"
    )
}

fn symbol_end(bytes: &[u8], index: usize) -> usize {
    for width in [3, 2] {
        let Some(candidate) = bytes.get(index..index + width) else {
            continue;
        };
        if matches!(
            candidate,
            b"<=>"
                | b"->>"
                | b">="
                | b"<="
                | b"<>"
                | b"!="
                | b"&&"
                | b"||"
                | b"<<"
                | b">>"
                | b"->"
                | b":="
        ) {
            return index + width;
        }
    }
    index + 1
}

fn lower_ascii(value: &[u8]) -> String {
    String::from_utf8(value.iter().map(u8::to_ascii_lowercase).collect())
        .expect("CHECK token normalization preserves UTF-8")
}

fn strip_redundant_outer_parentheses(tokens: &mut Vec<CheckToken>) {
    while is_wrapped_by_single_outer_group(tokens) {
        tokens.remove(0);
        tokens.pop();
    }
}

fn is_wrapped_by_single_outer_group(tokens: &[CheckToken]) -> bool {
    if !matches!(tokens.first(), Some(CheckToken::OpenParenthesis))
        || !matches!(tokens.last(), Some(CheckToken::CloseParenthesis))
    {
        return false;
    }
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        if matches!(token, CheckToken::OpenParenthesis) {
            depth += 1;
        } else if matches!(token, CheckToken::CloseParenthesis) {
            let Some(next_depth) = depth.checked_sub(1) else {
                return false;
            };
            depth = next_depth;
            if depth == 0 && index + 1 != tokens.len() {
                return false;
            }
        }
    }
    depth == 0
}

fn strip_redundant_atomic_parentheses(tokens: &mut Vec<CheckToken>) {
    // MySQL 会给比较项补上括号。只移除逻辑表达式之外的分组；IN 列表、函数调用和
    // 含 AND/OR 的组继续保留，避免把具有不同优先级的表达式归为同一约束。
    while let Some((open, close)) = redundant_atomic_group(tokens) {
        tokens.remove(close);
        tokens.remove(open);
    }
}

fn redundant_atomic_group(tokens: &[CheckToken]) -> Option<(usize, usize)> {
    let mut opens = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        match token {
            CheckToken::OpenParenthesis => opens.push(index),
            CheckToken::CloseParenthesis => {
                let open = opens.pop()?;
                if atomic_group(tokens, open, index) {
                    return Some((open, index));
                }
            }
            _ => {}
        }
    }
    None
}

fn atomic_group(tokens: &[CheckToken], open: usize, close: usize) -> bool {
    let preceding = open.checked_sub(1).and_then(|index| tokens.get(index));
    if matches!(preceding, Some(CheckToken::Identifier(_)))
        || matches!(preceding, Some(CheckToken::Keyword(value)) if value == "in")
    {
        return false;
    }
    let mut depth = 0usize;
    let mut between_range = false;
    for token in &tokens[open + 1..close] {
        match token {
            CheckToken::OpenParenthesis => depth += 1,
            CheckToken::CloseParenthesis => depth = depth.saturating_sub(1),
            CheckToken::Keyword(value) if depth == 0 && value == "between" => between_range = true,
            CheckToken::Keyword(value) if depth == 0 && value == "and" && between_range => {
                between_range = false;
            }
            CheckToken::Keyword(value) if depth == 0 && matches!(value.as_str(), "and" | "or") => {
                return false;
            }
            CheckToken::Symbol(value) if depth == 0 && value == "," => return false,
            _ => {}
        }
    }
    true
}

fn encode_check_tokens(tokens: &[CheckToken]) -> String {
    let mut encoded = String::new();
    for token in tokens {
        let (kind, value) = match token {
            CheckToken::Identifier(value) => ('i', value.as_str()),
            CheckToken::Keyword(value) => ('k', value.as_str()),
            CheckToken::Number(value) => ('n', value.as_str()),
            CheckToken::Literal(value) => ('l', value.as_str()),
            CheckToken::Symbol(value) => ('s', value.as_str()),
            CheckToken::OpenParenthesis => ('p', "("),
            CheckToken::CloseParenthesis => ('p', ")"),
        };
        encoded.push(kind);
        encoded.push_str(&value.len().to_string());
        encoded.push(':');
        encoded.push_str(value);
        encoded.push(';');
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::normalize_check_clause;

    #[test]
    fn check_normalization_matches_mysql_rendering_without_changing_literals() {
        assert_eq!(
            normalize_check_clause(r#"(`state` IN (_utf8mb4\'active\', _ascii\'failed\'))"#),
            normalize_check_clause("((state in ('active', 'failed')))")
        );
        assert_eq!(
            normalize_check_clause(r#"(`code` = _utf8mb4\'O\\\'Reilly\')"#),
            normalize_check_clause("`code` = 'O''Reilly'"),
        );
        assert_ne!(
            normalize_check_clause("`code` = 'A B'"),
            normalize_check_clause("`code` = 'a b'"),
        );
        assert_ne!(
            normalize_check_clause("((`a` AND `b`) OR `c`)"),
            normalize_check_clause("(`a` AND (`b` OR `c`))"),
        );
    }

    #[test]
    fn check_normalization_preserves_token_types_and_boundaries() {
        assert_ne!(
            normalize_check_clause("`a-b` > 0"),
            normalize_check_clause("`a` - `b` > 0"),
        );
        assert_ne!(
            normalize_check_clause("`a` IS NULL"),
            normalize_check_clause("`aisnull`"),
        );
        assert_ne!(
            normalize_check_clause("TRUE"),
            normalize_check_clause("`true`"),
        );
    }

    #[test]
    fn check_normalization_matches_mysql_parenthesized_comparisons() {
        assert_eq!(
            normalize_check_clause(
                "((`attempts` >= 0) and (`max_attempts` between 1 and 100) and (`claim_sequence` >= `attempts`))",
            ),
            normalize_check_clause(
                "`attempts` >= 0 AND `max_attempts` BETWEEN 1 AND 100 AND `claim_sequence` >= `attempts`"
            ),
        );
        assert_eq!(
            normalize_check_clause(
                "((`status` in (_utf8mb4\\'running\\',_utf8mb4\\'failed\\')) and (`completed_at` is null))"
            ),
            normalize_check_clause("`status` IN ('running', 'failed') AND `completed_at` IS NULL"),
        );
    }
}

//! 与原检查器一致的版本及渠道识别，不引入正则运行时依赖。

const CHANNELS: &[&str] = &["alpha", "beta", "rc", "pre", "preview", "nightly", "canary"];

// Python 正则的 Unicode 十进制数字及 IGNORECASE 特殊字母保持原匹配边界。
const DECIMAL_RANGES: &[(u32, u32)] = &[
    (0x30, 0x39),
    (0x660, 0x669),
    (0x6f0, 0x6f9),
    (0x7c0, 0x7c9),
    (0x966, 0x96f),
    (0x9e6, 0x9ef),
    (0xa66, 0xa6f),
    (0xae6, 0xaef),
    (0xb66, 0xb6f),
    (0xbe6, 0xbef),
    (0xc66, 0xc6f),
    (0xce6, 0xcef),
    (0xd66, 0xd6f),
    (0xde6, 0xdef),
    (0xe50, 0xe59),
    (0xed0, 0xed9),
    (0xf20, 0xf29),
    (0x1040, 0x1049),
    (0x1090, 0x1099),
    (0x17e0, 0x17e9),
    (0x1810, 0x1819),
    (0x1946, 0x194f),
    (0x19d0, 0x19d9),
    (0x1a80, 0x1a89),
    (0x1a90, 0x1a99),
    (0x1b50, 0x1b59),
    (0x1bb0, 0x1bb9),
    (0x1c40, 0x1c49),
    (0x1c50, 0x1c59),
    (0xa620, 0xa629),
    (0xa8d0, 0xa8d9),
    (0xa900, 0xa909),
    (0xa9d0, 0xa9d9),
    (0xa9f0, 0xa9f9),
    (0xaa50, 0xaa59),
    (0xabf0, 0xabf9),
    (0xff10, 0xff19),
    (0x104a0, 0x104a9),
    (0x10d30, 0x10d39),
    (0x11066, 0x1106f),
    (0x110f0, 0x110f9),
    (0x11136, 0x1113f),
    (0x111d0, 0x111d9),
    (0x112f0, 0x112f9),
    (0x11450, 0x11459),
    (0x114d0, 0x114d9),
    (0x11650, 0x11659),
    (0x116c0, 0x116c9),
    (0x11730, 0x11739),
    (0x118e0, 0x118e9),
    (0x11950, 0x11959),
    (0x11c50, 0x11c59),
    (0x11d50, 0x11d59),
    (0x11da0, 0x11da9),
    (0x11f50, 0x11f59),
    (0x16a60, 0x16a69),
    (0x16ac0, 0x16ac9),
    (0x16b50, 0x16b59),
    (0x1d7ce, 0x1d7ff),
    (0x1e140, 0x1e149),
    (0x1e2f0, 0x1e2f9),
    (0x1e4f0, 0x1e4f9),
    (0x1e950, 0x1e959),
    (0x1fbf0, 0x1fbf9),
];

fn fold(c: char) -> char {
    match c {
        'İ' | 'ı' => 'i',
        'ſ' => 's',
        'K' => 'k',
        _ => c.to_ascii_lowercase(),
    }
}

fn word(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, 'İ' | 'ı' | 'ſ' | 'K')
}

fn version_end(value: &str) -> Option<usize> {
    let mut index = usize::from(value.starts_with(['v', 'V']));
    for separator in ['.', '.', '-'] {
        let start = index;
        for c in value[index..].chars() {
            if !DECIMAL_RANGES
                .iter()
                .any(|(start, end)| (*start..=*end).contains(&u32::from(c)))
            {
                break;
            }
            index += c.len_utf8();
        }
        if start == index || !value[index..].starts_with(separator) {
            return None;
        }
        index += 1;
    }
    let rest: String = value[index..].chars().map(fold).collect();
    if !CHANNELS.iter().any(|channel| rest.starts_with(channel)) {
        return None;
    }
    for c in value[index..].chars() {
        if !word(c) && !matches!(c, '.' | '-') {
            break;
        }
        index += c.len_utf8();
    }
    Some(index)
}

pub(crate) fn is_version(value: &str) -> bool {
    version_end(value) == Some(value.len())
}

pub(crate) fn is_reference(value: &str) -> bool {
    is_version(value)
        || value
            .chars()
            .map(fold)
            .collect::<String>()
            .split(['.', '-'])
            .any(|part| CHANNELS.contains(&part))
}

pub(crate) fn literals(source: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut index = 0;
    while index < source.len() {
        if (index == 0 || !source[..index].chars().next_back().is_some_and(word))
            && let Some(rest) = source.get(index..)
            && let Some(end) = version_end(rest)
        {
            values.push(rest[..end].to_owned());
            index += end;
        } else {
            index += source[index..]
                .chars()
                .next()
                .expect("有效字符边界")
                .len_utf8();
        }
    }
    values
}

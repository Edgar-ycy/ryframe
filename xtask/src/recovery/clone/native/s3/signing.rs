use std::collections::BTreeMap;

use hmac::{KeyInit, Mac, SimpleHmac};
use sha2::{Digest, Sha256};
use url::Url;

use crate::Result;

const INVALID: &str = "S3 签名输入无效，请核对方法、URL、签名头及凭据";
const HEX: &[u8; 16] = b"0123456789ABCDEF";

pub(crate) struct SigningInput<'a> {
    pub(crate) method: &'a str,
    pub(crate) url: &'a Url,
    pub(crate) headers: &'a BTreeMap<String, String>,
    pub(crate) payload_hash: &'a str,
    pub(crate) access_key: &'a str,
    pub(crate) secret_key: &'a str,
    pub(crate) region: &'a str,
    pub(crate) amz_date: &'a str,
}

pub(crate) fn authorization(input: SigningInput<'_>) -> Result<String> {
    validate(&input)?;
    let (headers, signed_headers) = canonical_headers(&input)?;
    let canonical = format!(
        "{}\n{}\n{}\n{headers}\n{signed_headers}\n{}",
        input.method,
        canonical_path(input.url.path())?,
        canonical_query(input.url)?,
        input.payload_hash,
    );
    let date = &input.amz_date[..8];
    let scope = format!("{date}/{}/s3/aws4_request", input.region);
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{scope}\n{}",
        input.amz_date,
        hex::encode(Sha256::digest(canonical.as_bytes())),
    );
    let date_key = hmac(
        format!("AWS4{}", input.secret_key).as_bytes(),
        date.as_bytes(),
    )?;
    let region_key = hmac(&date_key, input.region.as_bytes())?;
    let service_key = hmac(&region_key, b"s3")?;
    let signing_key = hmac(&service_key, b"aws4_request")?;
    let signature = hex::encode(hmac(&signing_key, to_sign.as_bytes())?);
    Ok(format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope},SignedHeaders={signed_headers},Signature={signature}",
        input.access_key,
    ))
}

fn validate(input: &SigningInput<'_>) -> Result<()> {
    if !token(input.method)
        || !matches!(input.url.scheme(), "http" | "https")
        || input.url.host_str().is_none()
        || !input.url.username().is_empty()
        || input.url.password().is_some()
        || input.url.fragment().is_some()
        || !token(input.access_key)
        || input.secret_key.is_empty()
        || input.secret_key.chars().any(char::is_control)
        || input.region.is_empty()
        || !input
            .region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || !valid_date(input.amz_date)
        || !(input.payload_hash == "UNSIGNED-PAYLOAD"
            || (input.payload_hash.len() == 64
                && input
                    .payload_hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))))
    {
        return Err(INVALID.into());
    }
    Ok(())
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 16
        || bytes[8] != b'T'
        || bytes[15] != b'Z'
        || bytes
            .iter()
            .enumerate()
            .any(|(i, b)| i != 8 && i != 15 && !b.is_ascii_digit())
    {
        return false;
    }
    let number = |start, end| value[start..end].parse::<u32>().unwrap_or(0);
    let year = number(0, 4);
    let month = number(4, 6);
    let day = number(6, 8);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return false,
    };
    year != 0
        && (1..=days).contains(&day)
        && number(9, 11) < 24
        && number(11, 13) < 60
        && number(13, 15) < 60
}

fn canonical_headers(input: &SigningInput<'_>) -> Result<(String, String)> {
    let mut canonical = BTreeMap::new();
    for (name, value) in input.headers {
        if !token(name)
            || name.bytes().any(|b| b.is_ascii_uppercase())
            || name == "authorization"
            || value.bytes().any(|b| b != b'\t' && (b < 32 || b == 127))
        {
            return Err(INVALID.into());
        }
        // 仅计算 AWS 规范头，不修改调用者实际发送的头；全部输入头均参与签名。
        canonical.insert(
            name.as_str(),
            value.split_ascii_whitespace().collect::<Vec<_>>().join(" "),
        );
    }
    let host = input.url.host_str().ok_or(INVALID)?;
    let host = match input.url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    for (name, expected) in [
        ("host", host.as_str()),
        ("x-amz-date", input.amz_date),
        ("x-amz-content-sha256", input.payload_hash),
    ] {
        if canonical.get(name).map(String::as_str) != Some(expected) {
            return Err(INVALID.into());
        }
    }
    let names = canonical.keys().copied().collect::<Vec<_>>().join(";");
    let headers = canonical
        .iter()
        .map(|(name, value)| format!("{name}:{value}\n"))
        .collect();
    Ok((headers, names))
}

fn encoded_byte(output: &mut String, byte: u8) {
    if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
        output.push(char::from(byte));
    } else {
        output.push('%');
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 15)]));
    }
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn query_component(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes
                .get(index + 1)
                .copied()
                .and_then(hex_digit)
                .ok_or(INVALID)?;
            let low = bytes
                .get(index + 2)
                .copied()
                .and_then(hex_digit)
                .ok_or(INVALID)?;
            decoded.push(high * 16 + low);
            index += 3;
        } else {
            // 查询参数中的加号是字面字节，不能按表单规则转换为空格。
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    std::str::from_utf8(&decoded).map_err(|_| INVALID)?;
    let mut encoded = String::new();
    for byte in decoded {
        encoded_byte(&mut encoded, byte);
    }
    Ok(encoded)
}

fn canonical_query(url: &Url) -> Result<String> {
    let Some(raw) = url.query().filter(|raw| !raw.is_empty()) else {
        return Ok(String::new());
    };
    let mut pairs = Vec::new();
    for pair in raw.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        pairs.push((query_component(key)?, query_component(value)?));
    }
    pairs.sort_unstable();
    Ok(pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&"))
}

fn canonical_path(path: &str) -> Result<String> {
    let mut result = String::new();
    let bytes = path.as_bytes();
    let mut index = 0;
    // 不合并斜线、不处理点段；Url 解析前已丢失的点段无法由本函数恢复。
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let high = *bytes.get(index + 1).ok_or(INVALID)?;
                let low = *bytes.get(index + 2).ok_or(INVALID)?;
                if hex_digit(high).is_none() || hex_digit(low).is_none() {
                    return Err(INVALID.into());
                }
                result.push('%');
                result.push(char::from(high));
                result.push(char::from(low));
                index += 3;
            }
            b'/' => {
                result.push('/');
                index += 1;
            }
            byte => {
                encoded_byte(&mut result, byte);
                index += 1;
            }
        }
    }
    Ok(result)
}

fn hmac(key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    let mut mac = SimpleHmac::<Sha256>::new_from_slice(key).map_err(|_| "S3 签名密钥初始化失败")?;
    mac.update(data);
    Ok(mac.finalize().into_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const DATE: &str = "20130524T000000Z";
    const ACCESS: &str = "AKIAIOSFODNN7EXAMPLE";
    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

    fn headers(host: &str) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("host".into(), host.into()),
            ("x-amz-date".into(), DATE.into()),
            ("x-amz-content-sha256".into(), EMPTY.into()),
        ])
    }

    fn input<'a>(url: &'a Url, headers: &'a BTreeMap<String, String>) -> SigningInput<'a> {
        SigningInput {
            method: "GET",
            url,
            headers,
            payload_hash: EMPTY,
            access_key: ACCESS,
            secret_key: SECRET,
            region: "us-east-1",
            amz_date: DATE,
        }
    }

    #[test]
    fn aws_20130524_get_range_has_the_complete_published_signature() {
        let url = Url::parse("https://examplebucket.s3.amazonaws.com/test.txt").unwrap();
        let mut headers = headers("examplebucket.s3.amazonaws.com");
        headers.insert("range".into(), "bytes=0-9".into());
        let actual = authorization(input(&url, &headers)).unwrap();
        assert_eq!(
            actual,
            concat!(
                "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request,",
                "SignedHeaders=host;range;x-amz-content-sha256;x-amz-date,",
                "Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
            )
        );
    }

    #[test]
    fn aws_lifecycle_and_list_vectors_keep_empty_values_and_sorted_query() {
        for (query, expected) in [
            (
                "lifecycle",
                "fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543",
            ),
            (
                "prefix=J&max-keys=2",
                "34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7",
            ),
        ] {
            let url =
                Url::parse(&format!("https://examplebucket.s3.amazonaws.com/?{query}")).unwrap();
            assert!(
                authorization(input(&url, &headers("examplebucket.s3.amazonaws.com")))
                    .unwrap()
                    .ends_with(&format!("Signature={expected}"))
            );
        }
    }

    #[test]
    fn query_keeps_duplicates_empty_values_unicode_percent_and_literal_plus() {
        let url =
            Url::parse("https://example.test/?z=中&k=+&k=%20&k=%2B&k=%25&k=&acl&=empty").unwrap();
        assert_eq!(
            canonical_query(&url).unwrap(),
            "=empty&acl=&k=&k=%20&k=%25&k=%2B&k=%2B&z=%E4%B8%AD"
        );
        assert_eq!(
            canonical_query(&Url::parse("https://example.test/?").unwrap()).unwrap(),
            ""
        );
    }

    #[test]
    fn encoded_query_names_and_values_are_sorted_after_encoding() {
        let url = Url::parse("https://example.test/?a=z&a=a&~=1&中=2&%61=m").unwrap();
        assert_eq!(
            canonical_query(&url).unwrap(),
            "%E4%B8%AD=2&a=a&a=m&a=z&~=1"
        );
    }

    #[test]
    fn path_keeps_slashes_dot_segments_and_existing_percent_encoding() {
        assert_eq!(
            canonical_path("/bucket//./../a%20b/%2F/%252F").unwrap(),
            "/bucket//./../a%20b/%2F/%252F"
        );
        assert_eq!(
            canonical_path("/test$file.text").unwrap(),
            "/test%24file.text"
        );
        assert_eq!(canonical_path("/中文").unwrap(), "/%E4%B8%AD%E6%96%87");
        let url = Url::parse("https://example.test/bucket//a%20b/%2F").unwrap();
        assert_eq!(canonical_path(url.path()).unwrap(), url.path());
    }

    #[test]
    fn all_headers_including_if_match_are_sorted_folded_and_signed_without_mutation() {
        let url = Url::parse("https://example.test/key").unwrap();
        let mut headers = headers("example.test");
        headers.insert("if-match".into(), " \t\"etag\"\t ".into());
        headers.insert("x-example".into(), " one\t two   three ".into());
        let original = headers.clone();
        let (canonical, names) = canonical_headers(&input(&url, &headers)).unwrap();
        assert_eq!(
            names,
            "host;if-match;x-amz-content-sha256;x-amz-date;x-example"
        );
        assert!(canonical.contains("if-match:\"etag\"\nx-amz-content-sha256:"));
        assert!(canonical.ends_with("x-example:one two three\n"));
        let signature = authorization(input(&url, &headers)).unwrap();
        assert!(signature.contains(&format!("SignedHeaders={names},")));
        headers.insert("if-match".into(), "\"different-etag\"".into());
        assert_ne!(authorization(input(&url, &headers)).unwrap(), signature);
        assert_eq!(original["x-example"], " one\t two   three ");
    }

    #[test]
    fn host_includes_nondefault_ports_and_ipv6_brackets() {
        for (raw, host) in [
            ("http://127.0.0.1:19000/key", "127.0.0.1:19000"),
            ("https://[::1]:9443/key", "[::1]:9443"),
        ] {
            let url = Url::parse(raw).unwrap();
            authorization(input(&url, &headers(host))).unwrap();
            assert!(authorization(input(&url, &headers("example.test"))).is_err());
        }
    }

    #[test]
    fn illegal_or_uppercase_header_names_and_control_values_are_rejected() {
        let url = Url::parse("https://example.test/key").unwrap();
        for (name, value) in [
            ("Host", "example.test"),
            ("bad name", "x"),
            ("", "x"),
            ("bad:name", "x"),
            ("authorization", "x"),
            ("x-test", "x\r\ny"),
            ("x-test", "x\0y"),
            ("x-test", "x\u{7f}y"),
            ("x-test", "x\u{b}y"),
        ] {
            let mut values = headers("example.test");
            values.insert(name.into(), value.into());
            assert!(authorization(input(&url, &values)).is_err());
        }
    }

    #[test]
    fn mandatory_headers_and_date_payload_consistency_cannot_be_bypassed() {
        let url = Url::parse("https://example.test/key").unwrap();
        for name in ["host", "x-amz-date", "x-amz-content-sha256"] {
            let mut values = headers("example.test");
            values.remove(name);
            assert!(authorization(input(&url, &values)).is_err());
            values.insert(name.into(), "inconsistent".into());
            assert!(authorization(input(&url, &values)).is_err());
        }
    }

    #[test]
    fn dates_validate_shape_calendar_leap_days_and_time() {
        for date in [
            "",
            "20130524",
            "20130524T000000Z\r\n",
            "20130229T000000Z",
            "20131301T000000Z",
            "20130431T000000Z",
            "20130524T240000Z",
            "20130524T006000Z",
            "20130524T000060Z",
            "00000101T000000Z",
            "中文日期",
        ] {
            assert!(!valid_date(date));
        }
        assert!(valid_date("20000229T235959Z"));
        assert!(!valid_date("19000229T000000Z"));
    }

    #[test]
    fn malformed_percent_encodings_or_non_utf8_query_are_rejected() {
        for query in ["x=%", "x=%0", "x=%GG", "x=%FF"] {
            let url = Url::parse(&format!("https://example.test/?{query}")).unwrap();
            assert!(canonical_query(&url).is_err());
        }
        for path in ["/%", "/%0", "/%GG"] {
            assert!(canonical_path(path).is_err());
        }
    }

    #[test]
    fn invalid_request_or_credentials_fail_with_no_input_values_in_errors() {
        let headers = headers("example.test");
        for raw in [
            "https://user:private-value@example.test/key",
            "https://example.test/key#fragment",
            "ftp://example.test/key",
        ] {
            let url = Url::parse(raw).unwrap();
            let message = authorization(input(&url, &headers))
                .unwrap_err()
                .to_string();
            assert!(!message.contains("private-value") && !message.contains(SECRET));
        }
        let url = Url::parse("https://example.test/key").unwrap();
        for field in ["method", "access", "secret", "region", "payload", "date"] {
            let mut value = input(&url, &headers);
            match field {
                "method" => value.method = "GET\r\nprivate-value",
                "access" => value.access_key = "private-value/injected",
                "secret" => value.secret_key = "private-value\n",
                "region" => value.region = "private-value/injected",
                "payload" => value.payload_hash = "private-value",
                _ => value.amz_date = "private-value",
            }
            let message = authorization(value).unwrap_err().to_string();
            assert!(!message.contains("private-value") && !message.contains(SECRET));
        }
    }

    #[test]
    fn unsigned_payload_is_explicit_and_must_match_the_header() {
        let url = Url::parse("https://example.test/key").unwrap();
        let mut headers = headers("example.test");
        headers.insert("x-amz-content-sha256".into(), "UNSIGNED-PAYLOAD".into());
        let mut value = input(&url, &headers);
        value.payload_hash = "UNSIGNED-PAYLOAD";
        authorization(value).unwrap();
        assert!(authorization(input(&url, &headers)).is_err());
    }
}

pub(crate) fn parse_commit(bytes: &[u8]) -> Result<String, &'static str> {
    let Some(commit) = bytes.strip_suffix(b"\n") else {
        return Err("内容必须以单个 LF 结尾");
    };
    if commit.len() != 40
        || !commit
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err("内容必须是 40 位小写十六进制提交 SHA");
    }
    String::from_utf8(commit.to_vec()).map_err(|_| "提交 SHA 不是 UTF-8")
}

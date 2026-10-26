#[path = "../build_support.rs"]
mod build_support;

#[test]
fn build_source_accepts_only_exact_lowercase_sha_line() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(
        build_support::parse_commit(format!("{sha}\n").as_bytes()).unwrap(),
        sha
    );
    for invalid in [
        sha.as_bytes().to_vec(),
        format!("{}\n", sha.to_uppercase()).into_bytes(),
        format!("{sha}\r\n").into_bytes(),
        format!("{}\n", &sha[..39]).into_bytes(),
        format!("{}g\n", &sha[..39]).into_bytes(),
        format!("{sha}\nextra\n").into_bytes(),
    ] {
        assert!(build_support::parse_commit(&invalid).is_err());
    }
}

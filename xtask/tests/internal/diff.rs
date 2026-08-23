use super::diff::unified;

#[test]
fn unchanged_content_has_no_diff() {
    assert!(unified("same\n", "same\n", "generated/a.rs").is_empty());
}

#[test]
fn replacement_keeps_context_and_labels() {
    let diff = unified(
        "one\ntwo\nthree\nfour\nfive\n",
        "one\ntwo\nchanged\nfour\nfive\n",
        "generated/a.rs",
    );
    assert!(diff.contains("--- 工作区/generated/a.rs"));
    assert!(diff.contains("+++ 生成结果/generated/a.rs"));
    assert!(diff.contains("-three\n+changed"));
    assert!(diff.contains(" two"));
}

#[test]
fn new_file_uses_zero_length_old_hunk() {
    let diff = unified("", "first\nsecond\n", "generated/new.rs");
    assert!(diff.contains("@@ -0,0 +1,2 @@"));
    assert!(diff.contains("+first\n+second"));
}

#[test]
fn missing_terminal_newline_is_visible() {
    let diff = unified("value", "value\n", "generated/a.rs");
    assert!(diff.contains("\\ No newline at end of file"));
}

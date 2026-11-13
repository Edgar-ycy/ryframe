use crate::recovery::b0;

#[test]
fn embedded_stable_readiness_adapter_has_registered_digest_and_scope() {
    b0::validate_patch(b0::PATCH_BYTES).unwrap();
    let patch = std::str::from_utf8(b0::PATCH_BYTES).unwrap();
    for path in b0::PATHS {
        assert!(patch.contains(&format!("diff --git a/{path} b/{path}")));
    }
    assert_eq!(patch.matches("diff --git ").count(), b0::PATHS.len());
}

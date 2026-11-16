use sha2::{Digest, Sha256};

use crate::devex::BaselineContract;

#[test]
fn embedded_stable_readiness_adapter_has_registered_digest_and_scope() {
    let patch = BaselineContract::STABLE_READINESS_B0_ADAPTER_PATCH;
    assert_eq!(
        format!("sha256:{}", hex::encode(Sha256::digest(patch))),
        BaselineContract::STABLE_READINESS_B0_PATCH_SHA256
    );
    let patch = std::str::from_utf8(patch).unwrap();
    for path in BaselineContract::STABLE_READINESS_B0_ADAPTER_PATHS {
        assert!(patch.contains(&format!("diff --git a/{path} b/{path}")));
    }
    assert_eq!(
        patch.matches("diff --git ").count(),
        BaselineContract::STABLE_READINESS_B0_ADAPTER_PATHS.len()
    );
}

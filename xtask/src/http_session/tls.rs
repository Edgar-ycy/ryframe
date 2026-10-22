use rustls::crypto::CryptoProvider;

use super::SessionError;

pub(super) fn install() -> Result<(), SessionError> {
    let expected = rustls::crypto::aws_lc_rs::default_provider();
    if CryptoProvider::get_default().is_none() {
        // 并发安装的另一调用者也必须在后续完整匹配核验中通过。
        let _ = expected.clone().install_default();
    }
    if CryptoProvider::get_default().is_some_and(|actual| matches(actual, &expected)) {
        Ok(())
    } else {
        Err(SessionError::new("crypto_provider", None, None))
    }
}

pub(super) fn matches(left: &CryptoProvider, right: &CryptoProvider) -> bool {
    left.cipher_suites
        .iter()
        .map(|suite| suite.suite())
        .eq(right.cipher_suites.iter().map(|suite| suite.suite()))
        && left
            .kx_groups
            .iter()
            .map(|group| group.name())
            .eq(right.kx_groups.iter().map(|group| group.name()))
        && left.signature_verification_algorithms.supported_schemes()
            == right.signature_verification_algorithms.supported_schemes()
        && std::ptr::eq(left.secure_random, right.secure_random)
        && std::ptr::eq(left.key_provider, right.key_provider)
}

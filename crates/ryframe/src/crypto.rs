//! 进程级 TLS 密码学 provider 装配。

use rustls::crypto::CryptoProvider;
use ryframe_kernel::{AppError, AppResult};

/// 在创建数据库、Redis、HTTP 或对象存储客户端前安装唯一的 AWS-LC provider。
///
/// 相同配置的重复调用是幂等的；如果进程中已经安装了不同 provider 或自定义配置，
/// 则拒绝继续启动，避免不同客户端静默使用不同密码学实现。
pub fn install_crypto_provider() -> AppResult<()> {
    let expected = rustls::crypto::aws_lc_rs::default_provider();
    if let Some(installed) = CryptoProvider::get_default() {
        return ensure_expected_provider(installed, &expected);
    }

    match expected.clone().install_default() {
        Ok(()) => Ok(()),
        Err(_) => CryptoProvider::get_default().map_or_else(
            || {
                Err(AppError::Config(
                    "TLS 密码学 provider 并发安装后不可用".into(),
                ))
            },
            |installed| ensure_expected_provider(installed, &expected),
        ),
    }
}

fn ensure_expected_provider(
    installed: &CryptoProvider,
    expected: &CryptoProvider,
) -> AppResult<()> {
    if providers_match(installed, expected) {
        Ok(())
    } else {
        Err(AppError::Config(
            "进程已安装非 RyFrame AWS-LC 配置的 TLS 密码学 provider".into(),
        ))
    }
}

fn providers_match(left: &CryptoProvider, right: &CryptoProvider) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_accepts_the_complete_aws_lc_provider() {
        let expected = rustls::crypto::aws_lc_rs::default_provider();
        assert!(providers_match(&expected, &expected.clone()));

        let mut changed = expected.clone();
        changed.cipher_suites.clear();
        assert!(!providers_match(&changed, &expected));
        assert!(ensure_expected_provider(&changed, &expected).is_err());
    }
}

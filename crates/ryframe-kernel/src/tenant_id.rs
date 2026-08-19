use crate::{AppError, AppResult};

/// 已校验且借用原始存储的租户标识，避免仅为校验分配新字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TenantId<'a>(&'a str);

impl<'a> TenantId<'a> {
    /// 校验并借用租户标识。
    pub fn parse(value: &'a str) -> AppResult<Self> {
        let bytes = value.as_bytes();
        let is_alphanumeric = |byte: u8| byte.is_ascii_alphanumeric();
        if !(2..=64).contains(&bytes.len())
            || !bytes.first().is_some_and(|byte| is_alphanumeric(*byte))
            || !bytes.last().is_some_and(|byte| is_alphanumeric(*byte))
            || !bytes
                .iter()
                .all(|byte| is_alphanumeric(*byte) || matches!(byte, b'-' | b'_'))
        {
            return Err(AppError::Validation(
                "tenant ID must be 2-64 ASCII letters, digits, hyphens, or underscores and start/end with a letter or digit"
                    .into(),
            ));
        }
        Ok(Self(value))
    }

    /// 返回原始租户标识引用。
    pub const fn as_str(self) -> &'a str {
        self.0
    }
}

impl<'a> TryFrom<&'a str> for TenantId<'a> {
    type Error = AppError;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

#[cfg(test)]
mod tests {
    use super::TenantId;

    #[test]
    fn accepts_bounded_ascii_identifiers_without_allocating() {
        let source = String::from("tenant_01-a");
        let tenant_id = TenantId::parse(&source).unwrap();

        assert_eq!(tenant_id.as_str(), source);
        assert_eq!(tenant_id.as_str().as_ptr(), source.as_ptr());
    }

    #[test]
    fn rejects_unsafe_or_ambiguous_identifiers() {
        for value in ["a", "-tenant", "tenant-", "租户", "tenant:*", "a b"] {
            assert!(
                TenantId::parse(value).is_err(),
                "{value} should be rejected"
            );
        }
    }
}

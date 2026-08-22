pub mod jwt;
pub mod password;
pub mod permission;
pub mod principal;
pub mod rbac;
mod scope_digest;

pub use principal::RequestPrincipal;
pub use scope_digest::stable_scope_digest;

/// 比较安全敏感字节，避免在首个差异处提前返回。
#[must_use]
pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= usize::from(left.get(index).copied().unwrap_or(0))
            ^ usize::from(right.get(index).copied().unwrap_or(0));
    }
    difference == 0
}

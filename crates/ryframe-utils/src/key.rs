use sha2::{Digest, Sha256};

/// 为缓存、限流器或锁键中由攻击者控制的值构建固定长度且抗碰撞的摘要。
/// 计算哈希前会加入长度前缀，以消除元组边界歧义。
pub fn stable_scope_digest(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hex::encode(hasher.finalize())
}

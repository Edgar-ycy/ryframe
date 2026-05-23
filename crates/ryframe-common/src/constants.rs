// 全局常量（状态码、缓存键等）

/// 默认分页大小
pub const DEFAULT_PAGE_SIZE: u64 = 10;

/// 最大分页大小（防止客户端传入过大值导致性能问题）
pub const MAX_PAGE_SIZE: u64 = 100;

/// Redis 缓存键前缀
pub const CACHE_KEY_PREFIX: &str = "ryframe:";

/// 验证码缓存键前缀
pub const CAPTCHA_KEY_PREFIX: &str = "ryframe:captcha:";

/// 系统超级管理员角色标识
pub const SUPER_ADMIN_ROLE: &str = "admin";

/// 默认密码（新建用户时的初始密码）
pub const DEFAULT_PASSWORD: &str = "123456";

/// 请求 ID 响应头名称
pub const REQUEST_ID_HEADER: &str = "X-Request-Id";

/// 认证令牌请求头名称
pub const AUTH_TOKEN_HEADER: &str = "Authorization";

/// 令牌前缀（Bearer 方案）
pub const TOKEN_PREFIX: &str = "Bearer ";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants_valid() {
        const { assert!(DEFAULT_PAGE_SIZE > 0 && DEFAULT_PAGE_SIZE <= MAX_PAGE_SIZE) };
        assert!(CAPTCHA_KEY_PREFIX.starts_with(CACHE_KEY_PREFIX));
        assert!(TOKEN_PREFIX.ends_with(' '));
    }
}

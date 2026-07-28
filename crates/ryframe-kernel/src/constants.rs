/// 默认分页大小。
pub const DEFAULT_PAGE_SIZE: u64 = 10;

/// 单页最大记录数。
pub const MAX_PAGE_SIZE: u64 = 1000;

/// Redis 缓存键前缀。
pub const CACHE_KEY_PREFIX: &str = "ryframe:";

/// 验证码缓存键前缀。
pub const CAPTCHA_KEY_PREFIX: &str = "ryframe:captcha:";

/// 系统超级管理员角色标识。
pub const SUPER_ADMIN_ROLE: &str = "admin";

/// 请求 ID 响应头名称。
pub const REQUEST_ID_HEADER: &str = "X-Request-Id";

/// 认证令牌请求头名称。
pub const AUTH_TOKEN_HEADER: &str = "Authorization";

/// Bearer 令牌前缀。
pub const TOKEN_PREFIX: &str = "Bearer ";

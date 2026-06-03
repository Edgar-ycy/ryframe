use chrono::{DateTime, Utc};

// ============================================================
// 填充策略
// ============================================================
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillStrategy {
    /// 仅在插入时填充
    Insert,
    /// 仅在更新时填充
    Update,
    /// 插入和更新时都填充
    All,
}

// ============================================================
// 填充来源
// ============================================================
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillSource {
    /// 当前时间 chrono::Utc::now()
    Now,
    /// 当前用户 ID（预留，取自 FillContext）
    UserId,
    /// 当前用户名（预留，取自 FillContext）
    Username,
}

// ============================================================
// 默认填充规则表（唯一定义点，proc macro 引用此处）
// ============================================================
pub struct DefaultRule {
    pub field_name: &'static str,
    pub strategy: FillStrategy,
    pub source: FillSource,
}

/// 默认自动填充字段：
/// - created_at / create_time → 插入时填当前时间
/// - updated_at / update_time → 插入 + 更新时填当前时间
pub const DEFAULTS: &[DefaultRule] = &[
    DefaultRule {
        field_name: "created_at",
        strategy: FillStrategy::Insert,
        source: FillSource::Now,
    },
    DefaultRule {
        field_name: "updated_at",
        strategy: FillStrategy::All,
        source: FillSource::Now,
    },
    DefaultRule {
        field_name: "create_time",
        strategy: FillStrategy::Insert,
        source: FillSource::Now,
    },
    DefaultRule {
        field_name: "update_time",
        strategy: FillStrategy::All,
        source: FillSource::Now,
    },
];

// ============================================================
// 填充上下文
// ============================================================
pub struct FillContext {
    /// 当前操作时间（统一时间戳，同一操作中 created_at == updated_at）
    pub now: DateTime<Utc>,
    /// 当前用户 ID（预留）
    pub user_id: Option<i64>,
    /// 当前用户名（预留）
    pub username: Option<String>,
}

impl FillContext {
    /// 创建上下文，时间戳自动取当前
    pub fn new() -> Self {
        Self {
            now: Utc::now(),
            user_id: None,
            username: None,
        }
    }
}

impl Default for FillContext {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 自动填充 trait
// ============================================================
pub trait AutoFill {
    /// 插入前自动填充
    fn fill_on_insert(&mut self, ctx: &FillContext);
    /// 更新前自动填充
    fn fill_on_update(&mut self, ctx: &FillContext);
}

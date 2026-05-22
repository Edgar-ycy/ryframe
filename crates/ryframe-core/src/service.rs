/// 业务 Service 标记 trait
///
/// 所有业务 Service 实现均应实现此 trait。
/// `Send + Sync` 保证 Service 可以跨线程安全共享（放入 Arc 注入 AppState）。
pub trait Service: Send + Sync {}
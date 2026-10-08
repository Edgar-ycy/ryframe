#[derive(ryframe_sdk::ResourceModel)]
#[resource(name = "order", title = "订单", table = "biz_order", database = "tenant")]
pub struct Order {
    #[resource(primary_key)]
    pub tenant_id: String,
    #[resource(primary_key, generated)]
    pub id: i64,
    #[resource(unique, filter, sort)]
    pub code: String,
    #[resource(read_only)]
    pub created_at: ryframe_sdk::chrono::DateTime<ryframe_sdk::chrono::Utc>,
    #[resource(read_only)]
    pub updated_at: ryframe_sdk::chrono::DateTime<ryframe_sdk::chrono::Utc>,
}

/// 登记验收模型；模块和进程 feature 由正式脚手架装配。
pub fn descriptors() -> Vec<ryframe_sdk::ResourceDescriptor> {
    vec![<Order as ryframe_sdk::ResourceModel>::descriptor()]
}

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

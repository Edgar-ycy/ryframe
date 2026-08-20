use chrono::{DateTime, Utc};

use crate::PersistenceFuture;

#[derive(Debug)]
pub struct ProductCapabilityRecord {
    pub code: String,
    pub variant: String,
    pub schema_version: i32,
    pub config: serde_json::Value,
}

#[derive(Debug)]
pub struct ProductVersionRecord {
    pub id: i64,
    pub version: i32,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub created_by: i64,
    pub published_by: Option<i64>,
    pub published_at: Option<DateTime<Utc>>,
    pub capabilities: Vec<ProductCapabilityRecord>,
}

#[derive(Debug)]
pub struct ProductPlanRecord {
    pub id: i64,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub created_by: i64,
    pub versions: Vec<ProductVersionRecord>,
}

pub trait ProductReadPort: Send + Sync {
    fn list_plans(&self) -> PersistenceFuture<'_, Vec<ProductPlanRecord>>;

    fn find_plan(&self, plan_id: i64) -> PersistenceFuture<'_, Option<ProductPlanRecord>>;
}

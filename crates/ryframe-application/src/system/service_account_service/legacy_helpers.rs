use chrono::{DateTime, Utc};
use ryframe_db::DataRetentionRepository;

use crate::PersistenceFuture;

pub(super) fn database_now<C>(database: &C) -> PersistenceFuture<'_, DateTime<Utc>>
where
    C: sea_orm::ConnectionTrait + Sync,
{
    Box::pin(async move { DataRetentionRepository.database_utc_now(database).await })
}

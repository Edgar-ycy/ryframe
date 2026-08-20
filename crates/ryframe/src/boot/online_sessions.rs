use std::sync::Arc;

use ryframe_adapters::RedisClient;
use ryframe_application::system::OnlineSessionMetadataStore;

mod keyspace;
mod redis_store;
mod session_codec;

pub fn redis_store(client: RedisClient) -> Arc<dyn OnlineSessionMetadataStore> {
    Arc::new(redis_store::RedisOnlineSessionMetadata::new(client))
}

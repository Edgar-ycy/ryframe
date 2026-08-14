use super::*;

pub(super) fn truncate_error(error: &str) -> String {
    const MAX_ERROR_BYTES: usize = 8 * 1024;
    if error.len() <= MAX_ERROR_BYTES {
        return error.to_owned();
    }
    let mut end = MAX_ERROR_BYTES;
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &error[..end])
}

pub(super) async fn rollback_quietly(transaction: DatabaseTransaction) {
    if let Err(error) = transaction.rollback().await {
        tracing::warn!(error = %error, "failed to rollback background job lease transaction");
    }
}

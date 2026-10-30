use serde_json::Value;

use crate::Result;

mod access;
mod binding;
mod create;
mod model;
#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/sql_stream.rs"]
mod tests;

pub(crate) fn request(operation: &str, args: &Value) -> Result<Value> {
    match operation {
        "reference-sql-stream-create" => create::run(args),
        "reference-sql-stream-read" => access::read(args),
        "reference-sql-stream-close" => access::close(args),
        _ => Err("未知 SQL 流操作".into()),
    }
}

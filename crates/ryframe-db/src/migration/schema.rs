mod expected;
mod inspect;
mod normalize;
mod types;
mod verify;

pub(crate) use inspect::user_tables;
pub use normalize::{expected_extra, extract_column_type, normalize_column_type};
pub use verify::verify_current_schema;

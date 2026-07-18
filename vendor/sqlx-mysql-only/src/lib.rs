//! MySQL-only SQLx facade used by RyFrame.
//!
//! This preserves the public runtime API SeaORM consumes while deliberately
//! omitting the PostgreSQL, SQLite, `Any`, and compile-time macro dependencies.

pub use sqlx_core::acquire::Acquire;
pub use sqlx_core::arguments::{Arguments, IntoArguments};
pub use sqlx_core::column::{Column, ColumnIndex, ColumnOrigin};
pub use sqlx_core::connection::{ConnectOptions, Connection};
pub use sqlx_core::database::{self, Database};
pub use sqlx_core::describe::Describe;
pub use sqlx_core::executor::{Execute, Executor};
pub use sqlx_core::from_row::FromRow;
pub use sqlx_core::pool::{self, Pool};
#[doc(hidden)]
pub use sqlx_core::query::query_with_result as __query_with_result;
pub use sqlx_core::query::{query, query_with};
pub use sqlx_core::query_as::{query_as, query_as_with};
pub use sqlx_core::query_builder::{self, QueryBuilder};
#[doc(hidden)]
pub use sqlx_core::query_scalar::query_scalar_with_result as __query_scalar_with_result;
pub use sqlx_core::query_scalar::{query_scalar, query_scalar_with};
pub use sqlx_core::raw_sql::{RawSql, raw_sql};
pub use sqlx_core::row::Row;
pub use sqlx_core::sql_str::{AssertSqlSafe, SqlSafeStr, SqlStr};
pub use sqlx_core::statement::Statement;
pub use sqlx_core::transaction::Transaction;
pub use sqlx_core::type_info::TypeInfo;
pub use sqlx_core::types::Type;
pub use sqlx_core::value::{Value, ValueRef};
pub use sqlx_core::Either;

#[doc(inline)]
pub use sqlx_core::error::{self, Error, Result};

#[cfg(feature = "migrate")]
pub use sqlx_core::migrate;

#[cfg(feature = "mysql")]
#[doc(inline)]
pub use sqlx_mysql::{
    self as mysql, MySql, MySqlConnection, MySqlExecutor, MySqlPool, MySqlTransaction,
};

#[doc(hidden)]
#[cfg(feature = "migrate")]
pub use sqlx_core::testing;

#[doc(hidden)]
pub use sqlx_core::rt::test_block_on;

#[doc(hidden)]
pub use sqlx_core::rt as __rt;

/// Conversions between Rust and MySQL types.
pub mod types {
    pub use sqlx_core::types::*;
}

/// Values encoded for MySQL queries.
pub mod encode {
    pub use sqlx_core::encode::{Encode, IsNull};
}

pub use self::encode::Encode;

/// Values decoded from MySQL rows.
pub mod decode {
    pub use sqlx_core::decode::Decode;
}

pub use self::decode::Decode;

/// Runtime query types.
pub mod query {
    pub use sqlx_core::query::{Map, Query};
    pub use sqlx_core::query_as::QueryAs;
    pub use sqlx_core::query_scalar::QueryScalar;
}

/// Common SQLx runtime traits.
pub mod prelude {
    pub use super::{
        Acquire, ConnectOptions, Connection, Decode, Encode, Executor, FromRow, IntoArguments, Row,
        Statement, Type,
    };
}

// SemVer-exempt SQLx runtime configuration APIs used by upstream crates.
#[doc(hidden)]
pub mod _unstable {
    pub use sqlx_core::config;
}

#[doc(hidden)]
pub fn warn_on_ambiguous_inferred_date_time_crate() {}

#[doc(hidden)]
pub fn warn_on_ambiguous_inferred_numeric_crate() {}

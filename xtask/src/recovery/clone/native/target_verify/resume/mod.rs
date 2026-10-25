//! 目标续作的纯证据模型与状态转换；协议不执行远端副作用。
pub(super) mod inventory;
mod inventory_failure;
mod inventory_records;
mod protocol;
mod records;
mod session;
pub(super) mod transition;

pub(super) use protocol::{diagnostic, execute};

use super::super::{digest, json as json_io};
use super::evidence_model::*;
use crate::Result;
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

fn zero(value: &Value) -> bool {
    value.as_u64() == Some(0)
}

fn attempt(value: &Value) -> Result<&str> {
    let value = json_io::text(value, "续作 attempt")?;
    check(
        value.len() == 32
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "目标续作 attempt 无效",
    )?;
    Ok(value)
}

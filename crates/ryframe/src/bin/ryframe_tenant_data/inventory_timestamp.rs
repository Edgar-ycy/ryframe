use std::borrow::Cow;

use ryframe_application::ports::utc_timestamp;
use ryframe_kernel::{AppError, AppResult};
use serde::{Serialize, Serializer, ser::Error};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Clone, Copy, Debug)]
pub(super) struct InventoryTimestamp {
    timestamp: OffsetDateTime,
    leap_second: bool,
}

impl InventoryTimestamp {
    pub(super) fn parse(value: &str) -> AppResult<Self> {
        let invalid = || AppError::Validation("库存观察时间必须是 RFC3339".into());
        let bytes = value.as_bytes();
        if !matches!(bytes.get(10), Some(b'T' | b't' | b' ')) {
            return Err(invalid());
        }

        let leap_second = bytes.get(17..19) == Some(b"60");
        let offset_minus = value.len().checked_sub("−00:00".len()).filter(|&index| {
            index >= 19
                && value
                    .get(index..)
                    .is_some_and(|offset| offset.starts_with('−'))
        });
        let normalized = if leap_second || offset_minus.is_some() {
            let mut input = value.to_owned();
            if let Some(index) = offset_minus {
                input.replace_range(index..index + '−'.len_utf8(), "-");
            }
            if leap_second {
                // 以 59 秒作为承载，保留闰秒的小数及其在同一分钟内的顺序。
                input.replace_range(17..19, "59");
            }
            Cow::Owned(input)
        } else {
            Cow::Borrowed(value)
        };
        let timestamp = OffsetDateTime::parse(&normalized, &Rfc3339).map_err(|_| invalid())?;
        Ok(Self {
            timestamp,
            leap_second,
        })
    }

    pub(super) fn validate_not_after(self, captured_at: OffsetDateTime) -> AppResult<()> {
        let nanos = self.timestamp.nanosecond() + u32::from(self.leap_second) * 1_000_000_000;
        if (self.timestamp.unix_timestamp(), nanos)
            > (captured_at.unix_timestamp(), captured_at.nanosecond())
        {
            return Err(AppError::Validation(
                "库存观察开始时间不能在采集时间之后".into(),
            ));
        }
        Ok(())
    }
}

impl Serialize for InventoryTimestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if !self.leap_second {
            return utc_timestamp::serialize(&self.timestamp, serializer);
        }
        let mut timestamp =
            utc_timestamp::format_with_offset(&self.timestamp).map_err(S::Error::custom)?;
        let second = timestamp
            .find('T')
            .ok_or_else(|| S::Error::custom("库存观察时间缺少时间分隔符"))?
            + "T00:00:".len();
        timestamp.replace_range(second..second + 2, "60");
        timestamp.truncate(timestamp.len() - "+00:00".len());
        timestamp.push('Z');
        serializer.serialize_str(&timestamp)
    }
}

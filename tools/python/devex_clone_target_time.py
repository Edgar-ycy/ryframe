"""fresh 目标追加证据的带时区时间戳与因果顺序校验。"""
import datetime as dt


def parse_timestamp(value: object) -> dt.datetime:
    if not isinstance(value, str) or not value or value != value.strip():
        raise ValueError("fresh 目标证据时间戳必须是非空带时区文本")
    normalized = value[:-1] + "+00:00" if value.endswith("Z") else value
    try:
        parsed = dt.datetime.fromisoformat(normalized)
    except ValueError as error:
        raise ValueError("fresh 目标证据时间戳不是 ISO-8601") from error
    if parsed.tzinfo is None or parsed.utcoffset() is None:
        raise ValueError("fresh 目标证据时间戳必须包含 UTC 或明确偏移")
    return parsed.astimezone(dt.timezone.utc)


def timestamp(value: object) -> bool:
    try:
        parse_timestamp(value)
        return True
    except (TypeError, ValueError):
        return False


def ordered(*values: object) -> bool:
    try:
        parsed = [parse_timestamp(value) for value in values]
    except (TypeError, ValueError):
        return False
    return all(before <= after for before, after in zip(parsed, parsed[1:]))

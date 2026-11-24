use super::super::{FieldIr, ResourceIr};

fn clock(field: &FieldIr) -> String {
    let expression = field
        .sql_on_update
        .as_ref()
        .or(field.sql_default.as_ref())
        .expect("数据库时钟已经由 IR 校验");
    let precision = expression
        .strip_prefix("CURRENT_TIMESTAMP(")
        .and_then(|value| value.strip_suffix(')'))
        .map_or(0, |value| {
            value.parse().expect("DATETIME 精度已经由 IR 校验")
        });
    let factor = 10_u32.pow(9 - precision);
    let value = format!(
        "now.replace_nanosecond(now.nanosecond() / {factor} * {factor}).expect(\"截断后的纳秒在 0..=999_999_999 范围内\")"
    );
    if field.nullable {
        format!("Some({value})")
    } else {
        value
    }
}

pub(super) fn insert(resource: &ResourceIr) -> String {
    if !resource.fields.iter().any(FieldIr::database_default) {
        return String::new();
    }
    let fields = resource
        .fields
        .iter()
        .map(|field| {
            let value = if field.database_default() {
                clock(field)
            } else {
                format!("record.{}", field.name)
            };
            format!("            {}: {value},", field.name)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mutable = if resource.id_generation == crate::IdGeneration::AutoIncrement {
        "mut "
    } else {
        ""
    };
    format!(
        "        let now = time::OffsetDateTime::now_utc();\n        let {mutable}record = {}Record {{\n{fields}\n        }};\n",
        resource.pascal_name
    )
}

pub(super) fn update(resource: &ResourceIr) -> String {
    let fields = resource
        .fields
        .iter()
        .filter(|field| field.database_update())
        .collect::<Vec<_>>();
    if fields.is_empty() {
        return String::new();
    }
    let preserve = fields
        .iter()
        .map(|field| format!("        record.{0} = previous.{0};\n", field.name))
        .collect::<String>();
    let updates = fields
        .iter()
        .filter(|field| field.sql_on_update.is_some())
        .map(|field| format!("            record.{} = {};\n", field.name, clock(field)))
        .collect::<String>();
    let changed = if updates.is_empty() {
        String::new()
    } else {
        format!(
            "        if record != previous {{\n            let now = time::OffsetDateTime::now_utc();\n{updates}        }}\n"
        )
    };
    format!(
        "        let previous = self.lock_view().get(&record.id).cloned().ok_or_else(|| AppError::NotFound(\"资源不存在\".into()))?;\n        let mut record = record;\n{preserve}{changed}"
    )
}

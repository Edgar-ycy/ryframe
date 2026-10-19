use serde_json::Value;

/// arbitrary_precision 的 JSON Number 不能直接交给 TOML serializer 当作普通数值。
pub(crate) fn toml_text(value: &Value) -> String {
    fn convert(value: &Value) -> toml::Value {
        match value {
            Value::Bool(value) => toml::Value::Boolean(*value),
            Value::String(value) => toml::Value::String(value.clone()),
            Value::Number(value) => {
                toml::Value::Integer(value.as_i64().expect("配置夹具仅使用有限整数"))
            }
            Value::Array(values) => toml::Value::Array(values.iter().map(convert).collect()),
            Value::Object(values) => toml::Value::Table(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), convert(value)))
                    .collect(),
            ),
            Value::Null => panic!("TOML 配置夹具不得含 null"),
        }
    }
    toml::to_string(&convert(value)).unwrap()
}

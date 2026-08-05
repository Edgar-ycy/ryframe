pub(super) fn insert(table: &mut toml::Table, path: &[&str], value: toml::Value) {
    if path.is_empty() {
        return;
    }

    insert_inner(table, path, value);
}

fn insert_inner(table: &mut toml::Table, path: &[&str], value: toml::Value) {
    if path.len() == 1 {
        table.insert(path[0].to_string(), value);
        return;
    }

    let child = ensure_table(table, path[0]);
    insert_inner(child, &path[1..], value);
}

fn ensure_table<'a>(table: &'a mut toml::Table, key: &str) -> &'a mut toml::Table {
    let value = table
        .entry(key.to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if !value.is_table() {
        *value = toml::Value::Table(toml::Table::new());
    }
    let toml::Value::Table(table) = value else {
        unreachable!("table was initialized above");
    };
    table
}

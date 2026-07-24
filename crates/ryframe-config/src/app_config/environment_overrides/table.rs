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

#[cfg(test)]
mod tests {
    use super::insert;

    #[test]
    fn nested_insert_creates_missing_tables() {
        let mut table = toml::Table::new();

        insert(
            &mut table,
            &["database", "primary", "port"],
            toml::Value::Integer(3307),
        );

        assert_eq!(
            table["database"]["primary"]["port"],
            toml::Value::Integer(3307)
        );
    }

    #[test]
    fn nested_insert_replaces_non_table_parent_like_before() {
        let mut table =
            toml::Table::from_iter([("database".into(), toml::Value::String("legacy".into()))]);

        insert(
            &mut table,
            &["database", "primary", "host"],
            toml::Value::String("db.internal".into()),
        );

        assert_eq!(
            table["database"]["primary"]["host"],
            toml::Value::String("db.internal".into())
        );
    }

    #[test]
    fn empty_path_is_ignored() {
        let mut table = toml::Table::new();

        insert(&mut table, &[], toml::Value::Integer(1));

        assert!(table.is_empty());
    }
}

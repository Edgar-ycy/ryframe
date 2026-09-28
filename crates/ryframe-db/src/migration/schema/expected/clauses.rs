use sea_orm::DbErr;

/// 仅在建表主体的顶层逗号处分段，约束表达式和字符串中的换行不改变结构。
pub(super) fn table_clauses(statement: &str) -> Result<Vec<&str>, DbErr> {
    let mut clauses = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in statement.char_indices() {
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == delimiter {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' | '`' => quote = Some(character),
            '(' => {
                if depth == 0 {
                    start = Some(index + 1);
                }
                depth += 1;
            }
            ')' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    push_clause(&mut clauses, statement, start, index)?;
                    return Ok(clauses);
                }
            }
            ',' if depth == 1 => {
                push_clause(&mut clauses, statement, start, index)?;
                start = Some(index + 1);
            }
            _ => {}
        }
    }
    Err(DbErr::Custom(
        "canonical baseline contains an unclosed CREATE TABLE body".into(),
    ))
}

fn push_clause<'a>(
    clauses: &mut Vec<&'a str>,
    statement: &'a str,
    start: Option<usize>,
    end: usize,
) -> Result<(), DbErr> {
    let clause = start.map(|start| statement[start..end].trim());
    let Some(clause) = clause.filter(|clause| !clause.is_empty()) else {
        return Err(DbErr::Custom(
            "canonical baseline contains an empty CREATE TABLE clause".into(),
        ));
    };
    clauses.push(clause);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::table_clauses;

    #[test]
    fn nested_expressions_and_quoted_delimiters_stay_in_their_clause() {
        let statement = r#"CREATE TABLE `example` (
            `id` BIGINT NOT NULL,
            `value` VARCHAR(32) DEFAULT 'a, ) ''b' COMMENT '文字 \' (,',
            CONSTRAINT `ck_value` CHECK (
                `id` > 0 AND (`value` IN ('a,b', 'c'))),
            CONSTRAINT `fk_example`
                FOREIGN KEY (`id`)
                REFERENCES `parent` (`id`) ON DELETE CASCADE,
            PRIMARY KEY (
                `id`, `value`)
        ) ENGINE=InnoDB"#;
        let clauses = table_clauses(statement).unwrap();
        assert_eq!(clauses.len(), 5);
        assert!(clauses[1].ends_with("COMMENT '文字 \\' (,'"));
        assert!(clauses[2].starts_with("CONSTRAINT `ck_value` CHECK"));
        assert!(clauses[3].ends_with("ON DELETE CASCADE"));
        assert!(clauses[4].contains("`id`, `value`"));
    }

    #[test]
    fn incomplete_or_empty_bodies_fail_closed() {
        for statement in [
            "CREATE TABLE `example`",
            "CREATE TABLE `example` ()",
            "CREATE TABLE `example` (`id` BIGINT,)",
            "CREATE TABLE `example` (`value` VARCHAR(32) DEFAULT 'unclosed)",
            "CREATE TABLE `example` (`id` BIGINT, CHECK ((`id` > 0))",
        ] {
            assert!(table_clauses(statement).is_err(), "{statement}");
        }
    }
}

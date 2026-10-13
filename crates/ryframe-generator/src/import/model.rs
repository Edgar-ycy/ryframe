use serde::Serialize;

/// 既有表的稳定结构描述；不依赖数据库连接实现。
#[derive(Debug, Clone, Serialize)]
pub struct TableInfo {
    pub table_name: String,
    pub comment: Option<String>,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
    pub foreign_key_dependencies: Vec<String>,
    pub schema_canonical: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexInfo {
    pub name: String,
    pub unique: bool,
    pub index_type: String,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForeignKeyInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub referenced_table: String,
    pub referenced_columns: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub column_type: String,
    pub default: Option<String>,
    pub extra: String,
    pub rust_type: String,
    pub is_nullable: bool,
    pub is_primary_key: bool,
    pub is_unique: bool,
    pub is_auto_increment: bool,
    pub comment: Option<String>,
}

/// 将已检查的表结构转换为供开发者确认的 Rust `ResourceModel` 初稿。
///
/// 导入只建立一次模型起点：不会猜测路由、菜单、字段用途或业务规则。
pub fn rust_model_source(table: &TableInfo, database: &str) -> Result<String, String> {
    if !matches!(database, "control" | "tenant") {
        return Err("导入数据库范围只支持 control 或 tenant".into());
    }
    validate_model_keys(table, database)?;
    let model = pascal_case(&table.table_name)?;
    let title = table.comment.as_deref().unwrap_or(&table.table_name);
    let mut output = format!(
        "// 由 ryframe-generate import 生成；请确认字段用途和业务规则。\n#[derive(ryframe_sdk::ResourceModel)]\n#[resource(name = {:?}, title = {:?}, table = {:?}, database = {:?})]\npub struct {} {{\n",
        table.table_name, title, table.table_name, database, model
    );
    for column in &table.columns {
        if !valid_ident(&column.name) {
            return Err(format!(
                "字段 {} 不是可用 Rust 标识符；请显式设置字段映射，导入未写入源码",
                column.name
            ));
        }
        let mut attributes = Vec::new();
        if column.is_primary_key {
            attributes.push("primary_key");
        }
        if column.is_auto_increment {
            attributes.push("generated");
        }
        if !attributes.is_empty() {
            output.push_str(&format!("    #[resource({})]\n", attributes.join(", ")));
        }
        output.push_str(&format!("    pub {}: {},\n", column.name, column.rust_type));
    }
    output.push_str("}\n");
    Ok(output)
}

fn validate_model_keys(table: &TableInfo, database: &str) -> Result<(), String> {
    let primary = table
        .columns
        .iter()
        .filter(|column| column.is_primary_key)
        .map(|column| column.name.as_str())
        .collect::<Vec<_>>();
    if primary.is_empty() {
        return Err(format!(
            "表 {} 没有主键；请先设计并审阅主键迁移，不能直接生成可写 CRUD",
            table.table_name
        ));
    }
    if database == "tenant" && !primary.contains(&"tenant_id") {
        return Err(format!(
            "租户表 {} 缺少包含 tenant_id 的主键；请选择全局业务 --database control，或审阅补充租户字段和主键的迁移",
            table.table_name
        ));
    }
    if primary.len() > 1 && !(database == "tenant" && primary.len() == 2 && primary.contains(&"id"))
    {
        return Err(format!(
            "表 {} 的复合主键 {primary:?} 暂不支持标准 CRUD；请使用强类型业务扩展",
            table.table_name
        ));
    }
    Ok(())
}

fn valid_ident(value: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "_", "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod",
        "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super",
        "trait", "true", "type", "unsafe", "use", "where", "while", "abstract", "become", "box",
        "do", "final", "macro", "override", "priv", "typeof", "unsized", "virtual", "yield", "try",
    ];
    let mut chars = value.chars();
    !KEYWORDS.contains(&value)
        && matches!(chars.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn pascal_case(value: &str) -> Result<String, String> {
    let mut output = String::new();
    for part in value.split('_').filter(|part| !part.is_empty()) {
        let mut characters = part.chars();
        let Some(first) = characters.next() else {
            continue;
        };
        if !first.is_ascii_alphabetic() {
            return Err(format!("表名 {value} 无法转换为 Rust 模型名"));
        }
        output.extend(first.to_uppercase());
        output.push_str(characters.as_str());
    }
    if output.is_empty() {
        Err(format!("表名 {value} 无法转换为 Rust 模型名"))
    } else {
        Ok(output)
    }
}

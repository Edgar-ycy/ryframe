use serde::Serialize;

/// 既有表的稳定结构描述；不依赖数据库连接实现。
#[derive(Debug, Clone, Serialize)]
pub struct TableInfo {
    pub table_name: String,
    pub comment: Option<String>,
    pub collation: Option<String>,
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
    pub collation: Option<String>,
    pub default: Option<String>,
    pub extra: String,
    pub rust_type: String,
    pub is_nullable: bool,
    pub is_primary_key: bool,
    pub is_unique: bool,
    pub is_auto_increment: bool,
    pub comment: Option<String>,
}

impl TableInfo {
    /// 主键顺序来自 PRIMARY 索引，独立于物理列顺序；单列初稿可从字段标记确定。
    pub(super) fn primary_key_columns(&self) -> Result<Vec<&str>, String> {
        let marked = self
            .columns
            .iter()
            .filter(|column| column.is_primary_key)
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>();
        if marked.is_empty() {
            return Err(format!(
                "表 {} 没有主键；请先设计并审阅主键迁移，不能直接生成可写 CRUD",
                self.table_name
            ));
        }
        let indexes = self
            .indexes
            .iter()
            .filter(|index| index.name == "PRIMARY")
            .collect::<Vec<_>>();
        let keys = match indexes.as_slice() {
            [index] if index.unique => index.columns.iter().map(String::as_str).collect::<Vec<_>>(),
            [] if marked.len() == 1 => marked.clone(),
            _ => {
                return Err(format!(
                    "表 {} 缺少唯一 PRIMARY 索引顺序或重复声明；重新读取完整结构",
                    self.table_name
                ));
            }
        };
        let unique = keys.iter().collect::<std::collections::BTreeSet<_>>();
        if keys.is_empty()
            || unique.len() != keys.len()
            || unique != marked.iter().collect::<std::collections::BTreeSet<_>>()
        {
            return Err(format!(
                "表 {} 的 PRIMARY 索引与字段主键标记不一致；重新读取完整结构",
                self.table_name
            ));
        }
        Ok(keys)
    }
}

/// 将已检查的表结构转换为供开发者确认的 Rust `ResourceModel` 初稿。
///
/// 导入只建立一次模型起点：不会猜测路由、菜单、字段用途或业务规则。
pub fn rust_model_source(table: &TableInfo, database: &str) -> Result<String, String> {
    rust_model_source_named(table, database, &table.table_name)
}

pub(super) fn rust_model_source_named(
    table: &TableInfo,
    database: &str,
    name: &str,
) -> Result<String, String> {
    if !matches!(database, "control" | "tenant") {
        return Err("导入数据库范围只支持 control 或 tenant".into());
    }
    let primary_key = validate_model_keys(table, database)?;
    let id_column = primary_key
        .iter()
        .find(|key| **key != "tenant_id")
        .copied()
        .ok_or_else(|| format!("表 {} 缺少业务主键；请审阅主键设计", table.table_name))?;
    if id_column != "id" && table.columns.iter().any(|column| column.name == "id") {
        return Err(format!(
            "表 {} 的主键 {id_column} 映射为 id 时与已有列冲突；请显式设计字段映射",
            table.table_name
        ));
    }
    let model_key = |column: &str| {
        if column == id_column {
            "id".to_owned()
        } else {
            column.to_owned()
        }
    };
    for column in &table.columns {
        let field = model_key(&column.name);
        if !valid_ident(&field) {
            return Err(format!(
                "字段 {} 不是可用 Rust 标识符；请显式设置字段映射，导入未写入源码",
                column.name
            ));
        }
    }
    let model_keys = primary_key
        .iter()
        .map(|column| model_key(column))
        .collect::<Vec<_>>();
    if !valid_ident(name) {
        return Err(format!(
            "资源名 {name:?} 不是可用 Rust 标识符；调整 strip_table_prefix 或表名映射"
        ));
    }
    let model = pascal_case(name)?;
    let title = table.comment.as_deref().unwrap_or(&table.table_name);
    let collation = table
        .collation
        .as_ref()
        .map(|value| format!(", collation = {value:?}"))
        .unwrap_or_default();
    let mut output = format!(
        "// 由 ryframe-generate import 生成；请确认字段用途和业务规则。\n#[derive(ryframe_sdk::ResourceModel)]\n#[resource(name = {:?}, title = {:?}, table = {:?}, database = {:?}, primary_key = {:?}{collation})]\n",
        name, title, table.table_name, database, model_keys
    );
    for index in &table.indexes {
        if index.name == "PRIMARY" {
            continue;
        }
        if index.index_type != "BTREE"
            || index.columns.is_empty()
            || index
                .columns
                .iter()
                .any(|key| !table.columns.iter().any(|column| column.name == *key))
        {
            return Err(format!(
                "表 {} 的索引 {} 不能生成：只支持完整列的 BTREE 索引；审阅索引定义或编写明确迁移，导入未写入源码",
                table.table_name, index.name
            ));
        }
        let fields = index
            .columns
            .iter()
            .map(|column| model_key(column))
            .collect::<Vec<_>>();
        output.push_str(&format!(
            "#[resource(index(name = {:?}, fields = {:?}{}))]\n",
            index.name,
            fields,
            if index.unique { ", unique" } else { "" }
        ));
    }
    output.push_str(&format!("pub struct {model} {{\n"));
    for column in &table.columns {
        let field = if column.name == id_column {
            "id"
        } else {
            &column.name
        };
        let mut attributes = Vec::<String>::new();
        if field != column.name {
            attributes.push(format!("column = {:?}", column.name));
        }
        if column.is_primary_key {
            attributes.push("primary_key".into());
        }
        if column.is_auto_increment {
            attributes.push("auto_increment".into());
        }
        let defaults = super::defaults::from_column(column)?;
        for (name, value) in [
            ("default", defaults.literal),
            ("sql_default", defaults.expression),
            ("sql_on_update", defaults.on_update),
        ] {
            if let Some(value) = value {
                attributes.push(format!("{name} = {value:?}"));
            }
        }
        attributes.push(format!("sql_type = {:?}", column.column_type));
        if let Some(collation) = &column.collation {
            attributes.push(format!("collation = {collation:?}"));
        }
        if !attributes.is_empty() {
            output.push_str(&format!("    #[resource({})]\n", attributes.join(", ")));
        }
        output.push_str(&format!("    pub {field}: {},\n", column.rust_type));
    }
    output.push_str("}\n");
    Ok(output)
}

fn validate_model_keys<'a>(table: &'a TableInfo, database: &str) -> Result<Vec<&'a str>, String> {
    let primary = table.primary_key_columns()?;
    if database == "tenant" && !primary.contains(&"tenant_id") {
        return Err(format!(
            "租户表 {} 缺少包含 tenant_id 的主键；请选择全局业务 --database control，或审阅补充租户字段和主键的迁移",
            table.table_name
        ));
    }
    if primary.len() > 1
        && !(database == "tenant" && primary.len() == 2 && primary[0] == "tenant_id")
    {
        return Err(format!(
            "表 {} 的复合主键 {primary:?} 暂不支持标准 CRUD；租户主键须以 tenant_id 开头，不自动重排数据库索引，请审阅迁移或使用强类型业务扩展",
            table.table_name
        ));
    }
    Ok(primary)
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

pub(super) fn pascal_case(value: &str) -> Result<String, String> {
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

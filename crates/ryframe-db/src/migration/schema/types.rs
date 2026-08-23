use std::collections::BTreeMap;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ExpectedTable {
    pub(super) engine: String,
    pub(super) character_set: String,
    pub(super) collation: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ExpectedColumn {
    pub(super) column_type: String,
    pub(super) nullable: bool,
    pub(super) default: Option<String>,
    pub(super) extra: String,
    pub(super) character_set: Option<String>,
    pub(super) collation: Option<String>,
    pub(super) generation_expression: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ExpectedIndex {
    pub(super) unique: bool,
    pub(super) columns: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ExpectedForeignKey {
    pub(super) columns: Vec<String>,
    pub(super) referenced_table: String,
    pub(super) referenced_columns: Vec<String>,
    pub(super) update_rule: String,
    pub(super) delete_rule: String,
}

#[derive(Default)]
pub(super) struct ExpectedSchema {
    pub(super) tables: BTreeMap<String, ExpectedTable>,
    pub(super) columns: BTreeMap<(String, String), ExpectedColumn>,
    pub(super) indexes: BTreeMap<(String, String), ExpectedIndex>,
    pub(super) foreign_keys: BTreeMap<(String, String), ExpectedForeignKey>,
}

#[derive(Debug)]
pub(super) struct ActualTable {
    pub(super) engine: String,
    pub(super) character_set: String,
    pub(super) collation: String,
}

#[derive(Debug)]
pub(super) struct ActualColumn {
    pub(super) column_type: String,
    pub(super) nullable: bool,
    pub(super) default: Option<String>,
    pub(super) extra: String,
    pub(super) character_set: Option<String>,
    pub(super) collation: Option<String>,
    pub(super) generation_expression: String,
}

#[derive(Default)]
pub(super) struct ActualIndex {
    pub(super) unique: bool,
    pub(super) index_type: String,
    pub(super) visible: bool,
    pub(super) columns: Vec<(i64, String, Option<i64>)>,
}

#[derive(Default)]
pub(super) struct ActualForeignKey {
    pub(super) columns: Vec<(i64, String)>,
    pub(super) referenced_table: String,
    pub(super) referenced_columns: Vec<(i64, String)>,
    pub(super) update_rule: String,
    pub(super) delete_rule: String,
}

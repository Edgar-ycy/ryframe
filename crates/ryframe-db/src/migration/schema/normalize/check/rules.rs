// Rust 与 SQL 词法器共用 token 分类事实；顺序保持最长符号优先。
pub(super) const INTRODUCERS: &[&str] = &["_utf8mb4", "_ascii"];
pub(super) const SYMBOLS: &[&str] = &[
    "<=>", "->>", ">=", "<=", "<>", "!=", "&&", "||", "<<", ">>", "->", ":=",
];
pub(super) const KEYWORDS: &[&str] = &[
    "and", "as", "between", "binary", "case", "collate", "div", "else", "end", "escape", "false",
    "in", "interval", "is", "like", "member", "mod", "not", "null", "of", "or", "regexp", "rlike",
    "sounds", "then", "true", "unknown", "when", "xor",
];

//! RyFrame 五类开发命令的解析与帮助。

#[path = "cli/help.rs"]
mod help;
#[path = "cli/model.rs"]
mod model;
#[path = "cli/parse.rs"]
mod parse;

#[allow(unused_imports)]
pub(crate) use help::print_help;
pub(crate) use model::*;
pub(crate) use parse::parse;

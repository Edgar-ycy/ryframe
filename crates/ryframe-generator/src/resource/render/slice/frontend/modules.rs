use super::{ResourceIr, slash_header};

// 每个聚合层最多引入 24 个子模块，避免宽模型把入口再次撑大。
const FANOUT: usize = 24;

pub(super) struct Function<'a> {
    pub name: &'a str,
    pub parameters: &'a str,
    pub arguments: &'a str,
    pub result: &'a str,
    pub array: bool,
}

pub(super) struct Modules<'a> {
    pub resource: &'a ResourceIr,
    pub assets: Vec<(String, String)>,
}

impl Modules<'_> {
    pub fn add(&mut self, path: impl Into<String>, body: impl AsRef<str>) {
        self.assets.push((
            path.into(),
            format!("{}{}", slash_header(self.resource), body.as_ref()),
        ));
    }

    pub fn tree(
        &mut self,
        stem: &str,
        leaves: Vec<String>,
        functions: &[Function<'_>],
        imports: &str,
    ) {
        if leaves.len() == 1 {
            self.add(format!("{stem}.ts"), lift_imports(&leaves[0]));
            return;
        }
        let mut paths = Vec::new();
        for (index, body) in leaves.iter().enumerate() {
            let path = format!("{stem}/part-{index}");
            self.add(format!("{path}.ts"), body);
            paths.push(path);
        }
        let mut level = 0;
        while paths.len() > FANOUT {
            paths = paths
                .chunks(FANOUT)
                .enumerate()
                .map(|(index, children)| {
                    let path = format!("{stem}/group-{level}-{index}");
                    let nested_imports = imports
                        .replace("from '../", "from '../../")
                        .replace("from './", "from '../");
                    let body = combine(children, functions, &nested_imports, stem);
                    self.add(format!("{path}.ts"), body);
                    path
                })
                .collect();
            level += 1;
        }
        self.add(
            format!("{stem}.ts"),
            combine(
                &paths,
                functions,
                imports,
                stem.rsplit_once('/').map_or("", |(parent, _)| parent),
            ),
        );
    }
}

fn combine(paths: &[String], functions: &[Function<'_>], imports: &str, directory: &str) -> String {
    let mut output = imports.to_owned();
    for (index, path) in paths.iter().enumerate() {
        let prefix = if directory.is_empty() {
            String::new()
        } else {
            format!("{directory}/")
        };
        let path = path
            .strip_prefix(&prefix)
            .expect("聚合只引用同目录的生成模块");
        if functions.len() == 1 {
            let name = functions[0].name;
            output.push_str(&format!(
                "import {{ {name} as {name}{index} }} from './{path}'\n"
            ));
        } else {
            output.push_str("import {\n");
            for function in functions {
                let name = function.name;
                output.push_str(&format!("  {name} as {name}{index},\n"));
            }
            output.push_str(&format!("}} from './{path}'\n"));
        }
    }
    for function in functions {
        let Function {
            name,
            parameters,
            arguments,
            result,
            array,
        } = function;
        let (open, close) = if *array { ("[", "]") } else { ("{", "}") };
        output.push_str(&format!(
            "\nexport function {name}({parameters}){result} {{\n  return {open}\n"
        ));
        for index in 0..paths.len() {
            output.push_str(&format!("    ...{name}{index}({arguments}),\n"));
        }
        output.push_str(&format!("  {close}\n}}\n"));
    }
    output
}

fn lift_imports(body: &str) -> String {
    let mut named_import = false;
    body.lines()
        .map(|line| {
            let import_start = line.starts_with("import {") || line.starts_with("import type {");
            named_import |= import_start;
            let module_line = named_import
                && (import_start || line.starts_with("} from '"))
                && line.ends_with('\'');
            if module_line {
                named_import = false;
            }
            // 只提升生成的命名导入路径，标签及占位文案保持原始字节。
            if module_line && let Some((prefix, path)) = line.split_once(" from '../") {
                let relative = if path.starts_with("../") {
                    path.to_owned()
                } else {
                    format!("./{path}")
                };
                format!("{prefix} from '{relative}\n")
            } else {
                format!("{line}\n")
            }
        })
        .collect()
}

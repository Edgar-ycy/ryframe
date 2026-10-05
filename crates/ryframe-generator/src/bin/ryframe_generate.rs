use ryframe_generator::business::{
    BusinessGenerateOptions, generate_business_package, locate_business_package,
};

const USAGE: &str = "用法：\n  ryframe-generate resource --package <crate> [--model <Model>] [--write] [--sync-frontend]\n  ryframe-generate import --connection <name> --database <control|tenant> --package <crate> --table <table> [--write]";

fn main() {
    if let Err(error) = run(std::env::args().skip(1).collect()) {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err(USAGE.into());
    };
    match command {
        "resource" => resource(&args[1..]),
        "import" => Err("数据库导入需要使用 --features schema-import；当前入口尚未启用".into()),
        _ => Err(USAGE.into()),
    }
}

fn resource(args: &[String]) -> Result<(), String> {
    let parsed = Arguments::parse(args)?;
    let package = parsed.required("--package")?;
    let current_dir = std::env::current_dir().map_err(|error| format!("无法读取当前目录：{error}"))?;
    let workspace_root = locate_business_package(&current_dir, package)
        .map_err(|error| error.to_string())?
        .workspace_root;
    let report = generate_business_package(BusinessGenerateOptions {
        current_dir: &current_dir,
        package,
        model: parsed.value("--model"),
        write: parsed.flag("--write"),
    })
    .map_err(|error| error.to_string())?;
    for (kind, paths) in [
        ("create", &report.created),
        ("update", &report.updated),
        ("remove", &report.removed),
    ] {
        for path in paths {
            println!("{kind} {path}");
        }
    }
    println!(
        "生成{}：新增 {}，更新 {}，删除 {}，未变化 {}。",
        if parsed.flag("--write") { "完成" } else { "预览" },
        report.created.len(),
        report.updated.len(),
        report.removed.len(),
        report.unchanged.len(),
    );
    if parsed.flag("--sync-frontend") {
        sync_frontend(&workspace_root, parsed.flag("--write"))?;
    }
    Ok(())
}

fn sync_frontend(workspace: &std::path::Path, write: bool) -> Result<(), String> {
    if !write {
        return Err("--sync-frontend 必须与 --write 一起使用".into());
    }
    let frontend = workspace
        .parent()
        .map(|parent| parent.join("ryframe-vue3"))
        .filter(|path| path.join("package.json").is_file())
        .ok_or_else(|| "无法定位相邻的 ryframe-vue3 工作区".to_owned())?;
    let status = std::process::Command::new("corepack")
        .args(["pnpm", "generate", "--write"])
        .current_dir(frontend)
        .status()
        .map_err(|error| format!("无法启动前端正式生成入口：{error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("前端生成失败：{status}"))
    }
}

struct Arguments {
    values: Vec<(String, String)>,
    flags: Vec<String>,
}

impl Arguments {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut values = Vec::new();
        let mut flags = Vec::new();
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--write" | "--sync-frontend" => {
                    flags.push(args[index].clone());
                    index += 1;
                }
                "--package" | "--model" => {
                    let value = args.get(index + 1).ok_or_else(|| USAGE.to_owned())?;
                    values.push((args[index].clone(), value.clone()));
                    index += 2;
                }
                _ => return Err(USAGE.into()),
            }
        }
        Ok(Self { values, flags })
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn required(&self, name: &str) -> Result<&str, String> {
        self.value(name).ok_or_else(|| format!("缺少 {name}\n{USAGE}"))
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|value| value == name)
    }
}

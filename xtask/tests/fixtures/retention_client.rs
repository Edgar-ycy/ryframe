//! 隔离的原生 MySQL/导出进程替身；只读本次选项文件，不连接任何服务。
#![forbid(unsafe_code)]
use std::{env,io::{self,BufRead,Write},process::{Command,Stdio},time::Duration};

fn main(){
    let args=env::args().skip(1).collect::<Vec<_>>();
    if args==["--fixture-orphan"]{std::thread::sleep(Duration::from_secs(60));return;}
    let executable=env::current_exe().expect("当前夹具路径");
    let defaults=args.iter().find_map(|v|v.strip_prefix("--defaults-file=")).expect("选项文件");
    let credentials=std::fs::read_to_string(defaults).expect("受管选项文件");
    let credential=credentials=="[client]\npassword=\"fixture-only\"\n";
    if args.iter().any(|v|v=="--databases"){
        let mode=args.iter().find_map(|v|v.strip_prefix("--user=")).unwrap_or("");
        if mode=="failed"{print!("partial SQL");eprintln!("fixture-only");std::process::exit(7);}
        if mode=="incomplete"{println!("CREATE TABLE `biz_order` ();");return;}
        println!("CREATE TABLE `biz_order` ();");
        for _ in 0..3{println!("INSERT INTO `biz_order` VALUES (1);");}
        return;
    }
    for line in io::stdin().lock().lines(){
        let line=line.expect("输入行");
        if line.contains("fixture_crash"){eprintln!("fixture-only");std::process::exit(7);}
        if line.contains("fixture_wait"){std::thread::sleep(Duration::from_secs(60));}
        if line.contains("fixture_identity"){
            println!("{{\"pid\":{},\"credential\":{},\"environment\":{}}}",std::process::id(),credential,env::var_os("MYSQL_PWD").is_none() && env::var_os("APP_DATABASE_PASSWORD").is_none());
        }
        if line.contains("fixture_orphan"){
            let child=Command::new(&executable).arg("--fixture-orphan").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("后代进程");
            println!("{{\"pid\":{}}}",child.id());
            // 故意不等待后代，用于证明受管进程树完整回收。
            std::mem::forget(child);
        }
        if let Some(sentinel)=line.strip_prefix("SELECT '").and_then(|v|v.strip_suffix("';")){
            println!("{sentinel}");
        }
        io::stdout().flush().expect("输出刷新");
    }
}

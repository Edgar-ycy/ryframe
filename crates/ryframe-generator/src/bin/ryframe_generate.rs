fn main() {
    if let Err(error) =
        ryframe_generator::command::run_from_args(std::env::args().skip(1).collect())
    {
        eprintln!("生成失败：{error}");
        std::process::exit(2);
    }
}

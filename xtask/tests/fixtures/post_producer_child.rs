//! 只消费隔离测试的 NDJSON，记录真实管道收到的请求。
use std::{
    env,
    fs::File,
    io::{self, BufRead, Write},
    process::ExitCode,
};

fn main() -> io::Result<ExitCode> {
    let mut arguments = env::args_os().skip(1);
    let mode = arguments.next().expect("缺少部门替身模式");
    let mut requests = File::create_new(arguments.next().expect("缺少部门替身请求记录路径"))?;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    record_request(&mut input, &mut requests)?;

    if mode == "terminal" {
        writeln!(
            output,
            "{{\"id\":1,\"ok\":false,\"error_type\":\"NativeBridgeFailure\",\"status\":null}}"
        )?;
        output.flush()?;
        return Ok(ExitCode::from(17));
    }

    assert_eq!(mode, "logout");
    writeln!(output, "{{\"id\":1,\"ok\":true,\"data\":{{}}}}")?;
    output.flush()?;
    record_request(&mut input, &mut requests)?;
    writeln!(
        output,
        "{{\"id\":2,\"ok\":true,\"data\":{{\"closed\":true}}}}"
    )?;
    output.flush()?;
    Ok(ExitCode::SUCCESS)
}

fn record_request(input: &mut impl BufRead, trace: &mut File) -> io::Result<()> {
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "生产者请求缺失",
        ));
    }
    trace.write_all(line.as_bytes())
}

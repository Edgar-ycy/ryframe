//! 浏览器与 DevEx 夹具共享的限流配置读取协议。

use std::{env, ffi::OsString, io::Read};

#[path = "rate_limit/config.rs"]
mod config;
#[path = "rate_limit/protocol.rs"]
mod protocol;

#[allow(unused_imports)]
pub(crate) use config::{
    DEFAULT_API_WINDOW_SECS, DEFAULT_LOGIN_CAPACITY, LOGIN_RULE, RateLimitError, authority,
    load_app_table, login_budget_environment, login_rate_limit, settings,
};

const MARKER: &str = "__rate-limit";
const MAX_REQUEST_BYTES: usize = 1024 * 1024;

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let mut arguments = env::args_os().skip(1);
    if arguments.next()?.as_os_str() != MARKER {
        return None;
    }
    let arguments = arguments.collect::<Vec<_>>();
    let mut input = Vec::new();
    let read = std::io::stdin()
        .take((MAX_REQUEST_BYTES as u64) + 1)
        .read_to_end(&mut input);
    let (code, output) = match read {
        Ok(_) if input.len() <= MAX_REQUEST_BYTES => execute(&arguments, &input),
        Ok(_) => protocol::invalid_request(),
        Err(_) => protocol::invalid_request(),
    };
    println!("{output}");
    Some(code)
}

pub(crate) fn execute(arguments: &[OsString], input: &[u8]) -> (i32, String) {
    if input.len() > MAX_REQUEST_BYTES {
        return protocol::invalid_request();
    }
    protocol::execute(arguments, input)
}

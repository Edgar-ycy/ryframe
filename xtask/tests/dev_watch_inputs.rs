use std::error::Error;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[path = "../src/source_edit.rs"]
mod source_edit;
#[path = "../src/watch.rs"]
mod watch;
// 原有四个内部场景原样加载；新增场景由 watcher 的测试模块提供。
#[path = "internal/watch.rs"]
mod original_watch_tests;

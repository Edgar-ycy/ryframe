// 生成结果通过 include! 编译，匿名导入让依赖检查器也能识别其直接依赖。
use async_trait as _;
use chrono as _;

#[allow(dead_code)]
mod generated_device_use_case {
    include!("golden/device_use_case.golden");
}

#[test]
fn tracked_generated_use_case_is_compiled_by_rustc() {
    let query = generated_device_use_case::DevicePageQuery::new(1, 20, 100)
        .expect("生成的分页值对象应可使用");
    assert_eq!(query.page(), 1);
    assert_eq!(query.page_size(), 20);
}

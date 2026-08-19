use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=RYFRAME_BUILD_COMMIT");
    let build_commit = env::var("RYFRAME_BUILD_COMMIT")
        .unwrap_or_else(|_| "development".to_owned())
        .trim()
        .to_ascii_lowercase();
    if build_commit != "development"
        && (build_commit.len() != 40 || !build_commit.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        panic!("RYFRAME_BUILD_COMMIT 必须是完整的 40 位 Git 提交 SHA");
    }
    println!("cargo:rustc-env=RYFRAME_BUILD_COMMIT={build_commit}");
}

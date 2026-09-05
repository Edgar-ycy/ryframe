use super::{
    build::{cargo_arguments, frontend_arguments},
    cli::BuildProfile,
};

#[test]
fn release_is_the_default_build_profile_contract() {
    let release = cargo_arguments(BuildProfile::Release);
    assert!(release.iter().any(|argument| argument == "--release"));
    assert!(
        release
            .windows(2)
            .any(|pair| pair == ["--target-dir", "target/build"])
    );
    assert!(
        release
            .iter()
            .any(|argument| argument == "--no-default-features")
    );
    assert!(
        release
            .windows(2)
            .any(|pair| pair == ["--features", "bin-api,bin-worker"])
    );
}

#[test]
fn explicit_dev_profile_uses_cargo_default_profile() {
    let dev = cargo_arguments(BuildProfile::Dev);
    assert!(!dev.iter().any(|argument| argument == "--release"));
    assert_eq!(frontend_arguments(false), ["build"]);
    assert_eq!(frontend_arguments(true), ["build", "--real"]);
}

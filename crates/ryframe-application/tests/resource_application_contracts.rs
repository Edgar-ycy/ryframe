use ryframe_application::generated::{GeneratedPersistencePorts, GeneratedServices};

#[test]
fn generated_service_registry_fails_closed_when_ports_are_missing() {
    let error = GeneratedServices::try_new(GeneratedPersistencePorts::default())
        .err()
        .expect("缺少生成资源端口时必须失败关闭");
    assert!(error.to_string().contains("缺少持久化端口"));
}

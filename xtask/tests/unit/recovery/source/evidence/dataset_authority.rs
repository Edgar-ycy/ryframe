use super::execute;

#[test]
fn dataset_authority_refuses_write_authorization_and_unknown_arguments() {
    let writing = br#"{"operation":"restore-dataset-authority","arguments":{},"write":true}"#;
    let error = execute(writing).unwrap_err().to_string();
    assert!(error.contains("写入授权"));
    let unknown = br#"{"operation":"restore-dataset-authority","arguments":{"backend_dir":"unread","runtime_receipt":"unread","target_plan":"unread","extra":true},"write":false}"#;
    let error = execute(unknown).unwrap_err().to_string();
    assert!(error.contains("恢复数据集权威参数"));
}

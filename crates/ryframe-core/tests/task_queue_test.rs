use ryframe_core::TaskQueue;
/// task_queue 模块测试
/// 从 crates/ryframe-core/src/task_queue.rs 内联测试迁移
use ryframe_core::task_queue::TaskMessage;

#[tokio::test]
async fn test_enqueue_dequeue_memory() {
    let queue = TaskQueue::new(None, "test_queue");
    let task = TaskMessage {
        task_type: "send_email".into(),
        payload: r#"{"to":"test@example.com"}"#.into(),
        created_at: 1000,
        max_retries: 3,
        retry_count: 0,
    };

    queue.enqueue(&task).await.unwrap();

    let dequeued = queue.dequeue(1).await.unwrap().unwrap();
    assert_eq!(dequeued.task_type, "send_email");
    assert_eq!(dequeued.payload, r#"{"to":"test@example.com"}"#);
}

#[tokio::test]
async fn test_dequeue_timeout_memory() {
    let queue = TaskQueue::new(None, "test_timeout");
    let result = queue.dequeue(1).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_multiple_tasks_fifo_memory() {
    let queue = TaskQueue::new(None, "test_fifo");

    for i in 0..5 {
        queue
            .enqueue(&TaskMessage {
                task_type: format!("task_{}", i),
                payload: "".into(),
                created_at: i * 100,
                max_retries: 3,
                retry_count: 0,
            })
            .await
            .unwrap();
    }

    for i in 0..5 {
        let task = queue.dequeue(1).await.unwrap().unwrap();
        assert_eq!(task.task_type, format!("task_{}", i));
    }
}

#[tokio::test]
async fn test_queue_len_memory() {
    let queue = TaskQueue::new(None, "test_len");
    // 内存模式无法精确获取队列长度
    assert_eq!(queue.len().await.unwrap(), 0);

    queue
        .enqueue(&TaskMessage {
            task_type: "test".into(),
            payload: "".into(),
            created_at: 0,
            max_retries: 0,
            retry_count: 0,
        })
        .await
        .unwrap();

    // 入队后能正常出队即可
    let task = queue.dequeue(1).await.unwrap().unwrap();
    assert_eq!(task.task_type, "test");
}

#[tokio::test]
async fn test_task_message_serialization() {
    let task = TaskMessage {
        task_type: "test".into(),
        payload: r#"{"key":"value"}"#.into(),
        created_at: 1234567890,
        max_retries: 5,
        retry_count: 2,
    };

    let json = serde_json::to_string(&task).unwrap();
    let deserialized: TaskMessage = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.task_type, "test");
    assert_eq!(deserialized.payload, r#"{"key":"value"}"#);
    assert_eq!(deserialized.max_retries, 5);
    assert_eq!(deserialized.retry_count, 2);
}

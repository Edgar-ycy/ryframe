use axum::extract::ws::Message;
use ryframe_middleware::websocket::{WsManager, WsMessage};
use tokio::sync::mpsc;

#[tokio::test]
async fn test_manager_create() {
    let mgr = WsManager::new();
    assert_eq!(mgr.connection_count(), 0);
}

#[tokio::test]
async fn test_register_and_unregister() {
    let mgr = WsManager::new();
    let (tx, _rx) = mpsc::unbounded_channel();

    mgr.register("conn-1".into(), tx);
    assert_eq!(mgr.connection_count(), 1);

    mgr.unregister(&"conn-1".into());
    assert_eq!(mgr.connection_count(), 0);
}

#[tokio::test]
async fn test_join_and_leave_room() {
    let mgr = WsManager::new();
    let (tx, _rx) = mpsc::unbounded_channel();

    mgr.register("conn-1".into(), tx);
    mgr.join_room(&"conn-1".into(), "admin");
    assert_eq!(mgr.room_member_count("admin"), 1);

    mgr.leave_room(&"conn-1".into(), "admin");
    assert_eq!(mgr.room_member_count("admin"), 0);
}

#[tokio::test]
async fn test_send_to_connection() {
    let mgr = WsManager::new();
    let (tx, mut rx) = mpsc::unbounded_channel();

    mgr.register("conn-1".into(), tx);

    let sent = mgr.send_to_connection_text(&"conn-1".into(), "hello").await;
    assert!(sent);

    let received = rx.recv().await;
    assert!(received.is_some());
    if let Some(Message::Text(text)) = received {
        assert!(text.contains("hello"));
    }
}

#[tokio::test]
async fn test_send_to_room() {
    let mgr = WsManager::new();
    let (tx1, mut rx1) = mpsc::unbounded_channel();
    let (tx2, mut rx2) = mpsc::unbounded_channel();
    let (tx3, mut rx3) = mpsc::unbounded_channel();

    mgr.register("c1".into(), tx1);
    mgr.register("c2".into(), tx2);
    mgr.register("c3".into(), tx3);

    mgr.join_room(&"c1".into(), "test-room");
    mgr.join_room(&"c3".into(), "test-room");
    // c2 NOT in test-room

    let count = mgr.send_to_room_text("test-room", "room msg").await;
    assert_eq!(count, 2);

    assert!(rx1.recv().await.is_some());
    assert!(rx3.recv().await.is_some());
    // c2 should NOT receive
    assert!(rx2.try_recv().is_err());
}

#[tokio::test]
async fn test_broadcast() {
    let mgr = WsManager::new();
    let (tx1, mut rx1) = mpsc::unbounded_channel();
    let (tx2, mut rx2) = mpsc::unbounded_channel();

    mgr.register("c1".into(), tx1);
    mgr.register("c2".into(), tx2);

    let count = mgr.broadcast_text("all hands").await;
    assert_eq!(count, 2);

    assert!(rx1.recv().await.is_some());
    assert!(rx2.recv().await.is_some());
}

#[tokio::test]
async fn test_unregister_removes_from_rooms() {
    let mgr = WsManager::new();
    let (tx, _rx) = mpsc::unbounded_channel();

    mgr.register("conn-1".into(), tx);
    mgr.join_room(&"conn-1".into(), "admin");
    assert_eq!(mgr.room_member_count("admin"), 1);

    mgr.unregister(&"conn-1".into());
    assert_eq!(mgr.connection_count(), 0);
    assert_eq!(mgr.room_member_count("admin"), 0);
}

#[tokio::test]
async fn test_ws_message_json() {
    let msg = WsMessage::notification("test");
    let json = msg.to_json();
    let parsed: WsMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.msg_type, "notification");
    assert_eq!(parsed.content, "test");
}

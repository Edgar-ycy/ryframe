use std::{
    net::TcpListener,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

use super::dev::{CycleControl, available_ports, wait_probe_services_ready_until_controlled};

#[test]
fn revision_control_is_rechecked_within_one_hundred_ms_during_stalled_io() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let api_port = listener.local_addr().unwrap().port();
    let (_, worker_port) = available_ports().unwrap();
    let server = thread::spawn(move || {
        let (_stream, _) = listener.accept().unwrap();
        thread::sleep(Duration::from_millis(100));
    });
    let control_checks = AtomicUsize::new(0);
    let started = Instant::now();

    let control = wait_probe_services_ready_until_controlled(
        Instant::now() + Duration::from_secs(1),
        "API",
        "Worker",
        || Ok(()),
        || Ok(()),
        api_port,
        worker_port,
        || {
            if control_checks.fetch_add(1, Ordering::Relaxed) >= 4 {
                CycleControl::Superseded
            } else {
                CycleControl::Continue
            }
        },
    )
    .unwrap();
    let observed = started.elapsed();

    server.join().unwrap();
    assert_eq!(control, CycleControl::Superseded);
    assert!(control_checks.load(Ordering::Relaxed) >= 5);
    assert!(
        observed < Duration::from_millis(100),
        "同步探活不得让代次检查阻塞 100ms：{observed:?}"
    );
}

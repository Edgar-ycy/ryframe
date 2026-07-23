use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicUsize, Ordering},
    },
    thread,
};

use ryframe_common::AppError;
use ryframe_common::utils::snowflake::{Snowflake, SnowflakeError, try_next_snowflake_id};

const TEST_TIMESTAMP: i64 = 1_800_000_000_000;
const SEQUENCE_MASK: i64 = (1 << 12) - 1;

#[test]
fn test_snowflake_creation_and_validation() {
    assert!(Snowflake::new(0).is_ok());
    assert!(Snowflake::new(1023).is_ok());
    assert_eq!(
        Snowflake::new(-1).err().unwrap(),
        SnowflakeError::InvalidWorkerId { worker_id: -1 }
    );
    assert_eq!(
        Snowflake::new(1024).err().unwrap(),
        SnowflakeError::InvalidWorkerId { worker_id: 1024 }
    );
}

#[test]
fn sequence_exhaustion_returns_immediately_and_recovers_next_millisecond() {
    let timestamp = Arc::new(AtomicI64::new(TEST_TIMESTAMP));
    let clock = Arc::clone(&timestamp);
    let clock_reads = Arc::new(AtomicUsize::new(0));
    let reads = Arc::clone(&clock_reads);
    let sf = Snowflake::with_time_source(42, move || {
        reads.fetch_add(1, Ordering::Relaxed);
        clock.load(Ordering::Relaxed)
    })
    .unwrap();

    let ids = (0..4096)
        .map(|_| sf.try_next_id().expect("sequence should be available"))
        .collect::<Vec<_>>();

    assert_eq!(Snowflake::extract_timestamp(ids[0]), TEST_TIMESTAMP);
    assert_eq!(ids[0] & SEQUENCE_MASK, 0);
    assert_eq!(Snowflake::extract_timestamp(ids[4095]), TEST_TIMESTAMP);
    assert_eq!(ids[4095] & SEQUENCE_MASK, 4095);
    assert_eq!(
        sf.try_next_id().unwrap_err(),
        SnowflakeError::SequenceExhausted {
            timestamp: TEST_TIMESTAMP
        }
    );
    assert_eq!(
        clock_reads.load(Ordering::Relaxed),
        4097,
        "序列耗尽只读取一次时钟，不轮询或睡眠"
    );

    timestamp.store(TEST_TIMESTAMP + 1, Ordering::Relaxed);
    let recovered = sf.try_next_id().expect("next millisecond should recover");
    assert_eq!(Snowflake::extract_timestamp(recovered), TEST_TIMESTAMP + 1);
    assert_eq!(recovered & SEQUENCE_MASK, 0);
}

#[test]
fn persistent_clock_rollback_is_a_typed_error_without_polling() {
    let timestamp = Arc::new(AtomicI64::new(TEST_TIMESTAMP + 10));
    let clock = Arc::clone(&timestamp);
    let clock_reads = Arc::new(AtomicUsize::new(0));
    let reads = Arc::clone(&clock_reads);
    let sf = Snowflake::with_time_source(9, move || {
        reads.fetch_add(1, Ordering::Relaxed);
        clock.load(Ordering::Relaxed)
    })
    .unwrap();

    let before_rollback = sf.try_next_id().unwrap();
    timestamp.store(TEST_TIMESTAMP, Ordering::Relaxed);
    assert_eq!(
        sf.try_next_id().unwrap_err(),
        SnowflakeError::ClockMovedBackwards {
            last_timestamp: TEST_TIMESTAMP + 10,
            observed_timestamp: TEST_TIMESTAMP,
        }
    );
    assert_eq!(
        clock_reads.load(Ordering::Relaxed),
        2,
        "回拨只读取一次时钟，不轮询或睡眠"
    );

    timestamp.store(TEST_TIMESTAMP + 11, Ordering::Relaxed);
    let recovered = sf.try_next_id().unwrap();
    assert!(before_rollback < recovered);
    assert_eq!(Snowflake::extract_timestamp(recovered), TEST_TIMESTAMP + 11);
    assert_eq!(recovered & SEQUENCE_MASK, 0);
}

#[test]
fn concurrent_generation_is_unique_and_preserves_worker_id() {
    const THREADS: usize = 8;
    const IDS_PER_THREAD: usize = 2_000;

    let clock_reads = Arc::new(AtomicI64::new(TEST_TIMESTAMP));
    let reads = Arc::clone(&clock_reads);
    let sf = Arc::new(
        Snowflake::with_time_source(513, move || reads.fetch_add(1, Ordering::Relaxed)).unwrap(),
    );

    let handles = (0..THREADS)
        .map(|_| {
            let sf = Arc::clone(&sf);
            thread::spawn(move || {
                (0..IDS_PER_THREAD)
                    .map(|_| sf.try_next_id().expect("monotonic clock should generate"))
                    .collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();

    let ids = handles
        .into_iter()
        .flat_map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    let unique = ids.iter().copied().collect::<HashSet<_>>();

    assert_eq!(ids.len(), THREADS * IDS_PER_THREAD);
    assert_eq!(unique.len(), ids.len());
    assert!(
        ids.iter()
            .all(|id| Snowflake::extract_worker_id(*id) == 513)
    );
}

#[test]
fn timestamp_and_worker_id_can_be_extracted() {
    let sf = Snowflake::with_time_source(42, || TEST_TIMESTAMP).unwrap();
    let id = sf.try_next_id().unwrap();

    assert!(id > 0);
    assert_eq!(Snowflake::extract_timestamp(id), TEST_TIMESTAMP);
    assert_eq!(Snowflake::extract_worker_id(id), 42);
}

#[test]
fn global_generator_returns_distinct_ids() {
    assert_ne!(
        try_next_snowflake_id().unwrap(),
        try_next_snowflake_id().unwrap()
    );
}

#[test]
fn generation_failure_maps_to_a_retryable_service_error() {
    let error = AppError::from(SnowflakeError::SequenceExhausted {
        timestamp: TEST_TIMESTAMP,
    });

    assert!(matches!(
        error,
        AppError::ServiceUnavailable(message)
            if message == "ID 生成服务暂时不可用，请稍后重试"
    ));
}

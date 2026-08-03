use ryframe_config::PaginationConfig;
use ryframe_core::{PageResult, ValidatedPageQuery};

fn page_query(page: u64, page_size: u64) -> ValidatedPageQuery {
    ValidatedPageQuery::new(page, page_size, &PaginationConfig::default()).unwrap()
}

#[test]
fn test_page_query_uses_runtime_default() {
    let q = ValidatedPageQuery::from_optional(None, None, &PaginationConfig::default()).unwrap();
    assert_eq!(q.page(), 1);
    assert_eq!(q.page_size(), 10);
}

#[test]
fn test_page_query_offset() {
    let q = page_query(1, 10);
    assert_eq!(q.offset(), 0);

    let q = page_query(2, 10);
    assert_eq!(q.offset(), 10);

    let q = page_query(3, 20);
    assert_eq!(q.offset(), 40);

    assert!(ValidatedPageQuery::new(0, 10, &PaginationConfig::default()).is_err());
}

#[test]
fn test_page_query_rejects_invalid_values_instead_of_clamping() {
    let policy = PaginationConfig {
        default_page_size: 20,
        max_page_size: 100,
    };

    assert!(ValidatedPageQuery::from_optional(Some(1), Some(5_000), &policy).is_err());
    assert!(ValidatedPageQuery::from_optional(Some(1), Some(0), &policy).is_err());
    assert!(ValidatedPageQuery::from_optional(Some(0), Some(10), &policy).is_err());

    let query = ValidatedPageQuery::from_optional(Some(3), Some(25), &policy).unwrap();
    assert_eq!(query.page(), 3);
    assert_eq!(query.page_size(), 25);
}

#[test]
fn strict_pagination_acceptance_matrix() {
    let policy = PaginationConfig {
        default_page_size: 10,
        max_page_size: 100,
    };

    assert!(ValidatedPageQuery::new(0, 10, &policy).is_err());
    assert!(ValidatedPageQuery::new(1, 0, &policy).is_err());

    let minimum = ValidatedPageQuery::new(1, 1, &policy).unwrap();
    assert_eq!(minimum.page(), 1);
    assert_eq!(minimum.page_size(), 1);
    assert_eq!(minimum.offset(), 0);

    let maximum = ValidatedPageQuery::new(1, 100, &policy).unwrap();
    assert_eq!(maximum.page_size(), 100);
    assert!(ValidatedPageQuery::new(1, 101, &policy).is_err());

    let largest_safe_page = ValidatedPageQuery::new(u64::MAX, 1, &policy).unwrap();
    assert_eq!(largest_safe_page.offset(), u64::MAX - 1);
    assert!(ValidatedPageQuery::new(u64::MAX, 2, &policy).is_err());

    for invalid_policy in [
        PaginationConfig {
            default_page_size: 0,
            max_page_size: 100,
        },
        PaginationConfig {
            default_page_size: 10,
            max_page_size: 0,
        },
        PaginationConfig {
            default_page_size: 101,
            max_page_size: 100,
        },
    ] {
        assert!(ValidatedPageQuery::from_optional(None, None, &invalid_policy).is_err());
    }
}

#[test]
fn test_page_result_new() {
    let q = page_query(2, 10);
    let pr = PageResult::new(vec![1, 2, 3], 30u64, &q);
    assert_eq!(pr.records, vec![1, 2, 3]);
    assert_eq!(pr.total, 30);
    assert_eq!(pr.page, 2);
    assert_eq!(pr.page_size, 10);
}

#[test]
fn test_page_result_total_pages() {
    let q = page_query(1, 10);

    let pr = PageResult::new(vec![1; 10], 30u64, &q);
    assert_eq!(pr.total_pages(), 3);

    let pr = PageResult::new(vec![1; 10], 25u64, &q);
    assert_eq!(pr.total_pages(), 3);

    let pr = PageResult::new(vec![1; 10], 10u64, &q);
    assert_eq!(pr.total_pages(), 1);

    let pr = PageResult::new(Vec::<i32>::new(), 0u64, &q);
    assert_eq!(pr.total_pages(), 0);

    let pr = PageResult {
        records: vec![1],
        total: 10,
        page: 1,
        page_size: 0,
    };
    assert_eq!(pr.total_pages(), 0);
}

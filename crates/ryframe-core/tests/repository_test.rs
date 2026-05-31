use ryframe_core::{LoggedRepo, PageQuery, PageResult};

#[test]
fn test_page_query_default() {
    let q = PageQuery::default();
    assert_eq!(q.page, 1);
    assert_eq!(q.page_size, 10);
}

#[test]
fn test_page_query_offset() {
    let q = PageQuery {
        page: 1,
        page_size: 10,
    };
    assert_eq!(q.offset(), 0);

    let q = PageQuery {
        page: 2,
        page_size: 10,
    };
    assert_eq!(q.offset(), 10);

    let q = PageQuery {
        page: 3,
        page_size: 20,
    };
    assert_eq!(q.offset(), 40);

    let q = PageQuery {
        page: 0,
        page_size: 10,
    };
    assert_eq!(q.offset(), 0);
}

#[test]
fn test_page_query_normalize() {
    let q = PageQuery {
        page: 1,
        page_size: 5000,
    }
    .normalize(1000);
    assert_eq!(q.page_size, 1000);

    let q = PageQuery {
        page: 1,
        page_size: 0,
    }
    .normalize(1000);
    assert_eq!(q.page_size, 10);

    let q = PageQuery {
        page: 0,
        page_size: 10,
    }
    .normalize(1000);
    assert_eq!(q.page, 1);

    let q = PageQuery {
        page: 3,
        page_size: 25,
    }
    .normalize(1000);
    assert_eq!(q.page, 3);
    assert_eq!(q.page_size, 25);
}

#[test]
fn test_page_result_new() {
    let q = PageQuery {
        page: 2,
        page_size: 10,
    };
    let pr = PageResult::new(vec![1, 2, 3], 30u64, &q);
    assert_eq!(pr.records, vec![1, 2, 3]);
    assert_eq!(pr.total, 30);
    assert_eq!(pr.page, 2);
    assert_eq!(pr.page_size, 10);
}

#[test]
fn test_page_result_total_pages() {
    let q = PageQuery::default();

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

#[test]
fn test_logged_repo_new_and_deref() {
    let inner = 42i32;
    let logged = LoggedRepo::new(inner);
    assert_eq!(*logged, 42);
}

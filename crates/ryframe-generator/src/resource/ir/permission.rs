pub(super) fn is_permission(value: &str) -> bool {
    let segments = value.split(':').collect::<Vec<_>>();
    segments.len() == 3 && segments.into_iter().all(is_kebab_segment)
}

fn is_kebab_segment(value: &str) -> bool {
    !value.is_empty()
        && value.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

use sha2::{Digest, Sha256};

/// Build a fixed-length, collision-resistant digest for attacker-controlled
/// values used in cache, limiter, or lock keys. Length-prefixing makes the
/// tuple boundary unambiguous before hashing.
pub fn stable_scope_digest(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_boundaries_and_values_are_distinct() {
        assert_ne!(
            stable_scope_digest(&["ab", "c"]),
            stable_scope_digest(&["a", "bc"])
        );
        assert_eq!(
            stable_scope_digest(&["same"]),
            stable_scope_digest(&["same"])
        );
        assert_eq!(stable_scope_digest(&["value"]).len(), 64);
    }
}

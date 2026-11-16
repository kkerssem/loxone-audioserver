//! Short-lived cache of the last TuneIn globalsearch query for category drill-down (`query=arr`).

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

const TTL: Duration = Duration::from_secs(30);

struct CachedQuery {
    query: String,
    at: Instant,
}

static LAST_QUERY: Mutex<Option<CachedQuery>> = Mutex::new(None);

/// Remember the query from the most recent TuneIn globalsearch (category links use `arr`).
pub fn remember_tunein_query(query: &str) {
    let query = query.trim();
    if query.is_empty() {
        return;
    }
    if let Ok(mut guard) = LAST_QUERY.lock() {
        *guard = Some(CachedQuery {
            query: query.to_string(),
            at: Instant::now(),
        });
    }
}

/// Resolve `arr` drill-down to the cached globalsearch query, if still within TTL.
pub fn last_tunein_query() -> Option<String> {
    let guard = LAST_QUERY.lock().ok()?;
    let cached = guard.as_ref()?;
    if cached.at.elapsed() > TTL {
        return None;
    }
    Some(cached.query.clone())
}

#[cfg(test)]
pub fn clear_tunein_query_cache() {
    if let Ok(mut guard) = LAST_QUERY.lock() {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn reset() {
        clear_tunein_query_cache();
    }

    #[test]
    #[serial]
    fn remember_and_retrieve_within_ttl() {
        reset();
        remember_tunein_query("jazz fm");
        assert_eq!(last_tunein_query().as_deref(), Some("jazz fm"));
    }

    #[test]
    #[serial]
    fn empty_query_is_not_stored() {
        reset();
        remember_tunein_query("");
        assert!(last_tunein_query().is_none());
    }
}

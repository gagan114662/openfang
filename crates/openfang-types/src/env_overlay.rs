//! Thread-safe environment variable overlay.
//!
//! Replaces unsafe `std::env::set_var`/`remove_var` in async handlers with a
//! safe in-process HashMap. Reads check the overlay first, then fall back to
//! the real process environment.

use std::collections::HashMap;
use std::sync::RwLock;

static ENV_OVERLAY: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

fn with_map<F, R>(f: F) -> R
where
    F: FnOnce(&HashMap<String, String>) -> R,
{
    let guard = ENV_OVERLAY.read().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some(map) => f(map),
        None => f(&HashMap::new()),
    }
}

fn with_map_mut<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashMap<String, String>) -> R,
{
    let mut guard = ENV_OVERLAY.write().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    f(map)
}

/// Store a secret/env value in the thread-safe overlay (replaces `std::env::set_var`).
pub fn set_secret_env(key: &str, value: &str) {
    with_map_mut(|map| {
        map.insert(key.to_string(), value.to_string());
    });
}

/// Remove a secret/env value from the overlay (replaces `std::env::remove_var`).
pub fn remove_secret_env(key: &str) {
    with_map_mut(|map| {
        map.remove(key);
    });
}

/// Read an env value: checks overlay first, falls back to real environment.
pub fn get_env_or_overlay(key: &str) -> Option<String> {
    let overlay_val = with_map(|map| map.get(key).cloned());
    if overlay_val.is_some() {
        return overlay_val;
    }
    std::env::var(key).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_overlay_set_get_remove() {
        set_secret_env("TEST_OVERLAY_KEY_1", "hello");
        assert_eq!(
            get_env_or_overlay("TEST_OVERLAY_KEY_1"),
            Some("hello".to_string())
        );
        remove_secret_env("TEST_OVERLAY_KEY_1");
        assert_eq!(get_env_or_overlay("TEST_OVERLAY_KEY_1"), None);
    }

    #[test]
    fn test_overlay_takes_priority() {
        std::env::set_var("TEST_OVERLAY_PRIO", "from_env");
        set_secret_env("TEST_OVERLAY_PRIO", "from_overlay");
        assert_eq!(
            get_env_or_overlay("TEST_OVERLAY_PRIO"),
            Some("from_overlay".to_string())
        );
        remove_secret_env("TEST_OVERLAY_PRIO");
        assert_eq!(
            get_env_or_overlay("TEST_OVERLAY_PRIO"),
            Some("from_env".to_string())
        );
        std::env::remove_var("TEST_OVERLAY_PRIO");
    }

    #[test]
    fn test_falls_back_to_real_env() {
        std::env::set_var("TEST_OVERLAY_FALLBACK", "real");
        assert_eq!(
            get_env_or_overlay("TEST_OVERLAY_FALLBACK"),
            Some("real".to_string())
        );
        std::env::remove_var("TEST_OVERLAY_FALLBACK");
    }
}

use crate::error::{AppError, AppResult};
use crate::store::master::Master;

pub enum Outcome<T> {
    Inserted(T), // first time; result was just stored
    Cached(T),   // already-seen key; cached result rehydrated
}

/// Run `f`, store its serialized result under `key`. If `key` already exists
/// (UNIQUE conflict), do not call `f` — return the cached result.
pub fn run<T, F>(
    master: &Master,
    key: &str,
    command: &str,
    now_ms: i64,
    f: F,
) -> AppResult<Outcome<T>>
where
    F: FnOnce() -> AppResult<T>,
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    if let Some(cached) = master.get_idempotency(key)? {
        let v: T = serde_json::from_str(&cached)
            .map_err(|e| AppError::Internal(format!("idempotency cached parse: {e}")))?;
        return Ok(Outcome::Cached(v));
    }
    let result = f()?;
    let json = serde_json::to_string(&result)
        .map_err(|e| AppError::Internal(format!("idempotency serialize: {e}")))?;
    master.put_idempotency(key, command, &json, now_ms)?;
    Ok(Outcome::Inserted(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_call_inserts() {
        let m = Master::open_in_memory().unwrap();
        let r = run(&m, "k1", "cmd", 1, || Ok(42i64)).unwrap();
        match r {
            Outcome::Inserted(v) => assert_eq!(v, 42),
            Outcome::Cached(_) => panic!("expected Inserted"),
        }
    }

    #[test]
    fn second_call_returns_cached_without_running_f() {
        let m = Master::open_in_memory().unwrap();
        run(&m, "k1", "cmd", 1, || Ok::<i64, AppError>(42)).unwrap();
        let r: Outcome<i64> = run(&m, "k1", "cmd", 2, || -> AppResult<i64> {
            panic!("f must not be called on cached key")
        })
        .unwrap();
        match r {
            Outcome::Cached(v) => assert_eq!(v, 42),
            Outcome::Inserted(_) => panic!("expected Cached"),
        }
    }
}

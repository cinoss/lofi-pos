use crate::crypto::{Dek, Kek};
use crate::error::AppResult;
use crate::store::master::Master;

/// Get the DEK for `business_day`. If absent, generate a fresh DEK,
/// wrap with KEK, attempt to insert (idempotent). Then unwrap and return.
///
/// This handles the race where two callers both try to create — the second
/// caller's `put_day_key` returns false and they fall through to read the
/// row that the first caller wrote.
pub fn get_or_create(master: &Master, kek: &Kek, business_day: &str) -> AppResult<Dek> {
    if let Some(wrapped) = master.get_dek(business_day)? {
        return kek.unwrap(&wrapped);
    }
    let dek = Dek::new_random();
    let wrapped = kek.wrap(&dek)?;
    let inserted = master.put_dek(business_day, &wrapped, 0)?;
    if inserted {
        Ok(dek)
    } else {
        // Lost the race; another caller wrote first. Read theirs.
        let stored = master
            .get_dek(business_day)?
            .ok_or(crate::error::AppError::NotFound)?;
        kek.unwrap(&stored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_call_creates_dek() {
        let m = Master::open_in_memory().unwrap();
        let kek = Kek::new_random();
        let dek1 = get_or_create(&m, &kek, "2026-04-27").unwrap();
        let stored = m.get_dek("2026-04-27").unwrap().unwrap();
        let dek2 = kek.unwrap(&stored).unwrap();
        assert_eq!(dek1.as_bytes(), dek2.as_bytes());
    }

    #[test]
    fn second_call_returns_same_dek() {
        let m = Master::open_in_memory().unwrap();
        let kek = Kek::new_random();
        let d1 = get_or_create(&m, &kek, "2026-04-27").unwrap();
        let d2 = get_or_create(&m, &kek, "2026-04-27").unwrap();
        assert_eq!(d1.as_bytes(), d2.as_bytes());
    }

    #[test]
    fn different_days_yield_different_deks() {
        let m = Master::open_in_memory().unwrap();
        let kek = Kek::new_random();
        let d1 = get_or_create(&m, &kek, "2026-04-27").unwrap();
        let d2 = get_or_create(&m, &kek, "2026-04-28").unwrap();
        assert_ne!(d1.as_bytes(), d2.as_bytes());
    }
}

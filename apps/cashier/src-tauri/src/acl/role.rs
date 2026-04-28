use serde::{Deserialize, Serialize};

/// Staff role hierarchy. `Owner > Manager > Cashier > Staff`.
/// Stored as the lowercase string in `staff.role` (CHECK constraint pins these).
///
/// IMPORTANT: variant declaration order defines the privilege hierarchy
/// (lowest first). `derive(Ord)` ranks variants by declaration position,
/// so reordering silently inverts every `actor >= min` check in
/// `policy::check`. Add new roles only at the appropriate rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Staff,
    Cashier,
    Manager,
    Owner,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Staff => "staff",
            Role::Cashier => "cashier",
            Role::Manager => "manager",
            Role::Owner => "owner",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "staff" => Some(Role::Staff),
            "cashier" => Some(Role::Cashier),
            "manager" => Some(Role::Manager),
            "owner" => Some(Role::Owner),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering() {
        assert!(Role::Owner > Role::Manager);
        assert!(Role::Manager > Role::Cashier);
        assert!(Role::Cashier > Role::Staff);
    }

    #[test]
    fn parse_roundtrip() {
        for r in [Role::Staff, Role::Cashier, Role::Manager, Role::Owner] {
            assert_eq!(Role::parse(r.as_str()), Some(r));
        }
        assert_eq!(Role::parse("nope"), None);
    }

    #[test]
    fn ordinal_positions_pinned() {
        assert_eq!(Role::Staff as u8, 0);
        assert_eq!(Role::Cashier as u8, 1);
        assert_eq!(Role::Manager as u8, 2);
        assert_eq!(Role::Owner as u8, 3);
    }

    #[test]
    fn serde_lowercase() {
        let s = serde_json::to_string(&Role::Manager).unwrap();
        assert_eq!(s, "\"manager\"");
        let r: Role = serde_json::from_str("\"owner\"").unwrap();
        assert_eq!(r, Role::Owner);
    }
}

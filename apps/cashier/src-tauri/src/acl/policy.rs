use crate::acl::{Action, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    OverrideRequired(Role),
}

#[derive(Debug, Clone, Copy)]
pub struct PolicyCtx {
    pub discount_pct: Option<u32>,
    pub discount_threshold_pct: u32,
    pub within_cancel_grace: bool,
    pub is_self: bool,
}

impl Default for PolicyCtx {
    fn default() -> Self {
        Self {
            discount_pct: None,
            discount_threshold_pct: 10,
            within_cancel_grace: false,
            is_self: false,
        }
    }
}

/// Decide whether `actor` may perform `action` under `ctx`.
/// Mirrors the spec's permission matrix.
pub fn check(action: Action, actor: Role, ctx: PolicyCtx) -> Decision {
    use Action::*;
    use Decision::*;

    let allow_at = |min: Role| -> Decision {
        if actor >= min {
            Allow
        } else {
            OverrideRequired(min)
        }
    };

    match action {
        // unrestricted
        OpenSession | PlaceOrder | ListRooms | ListTables | ListProducts | ListActiveSessions
        | GetSession => Allow,

        // self-cancel within grace window
        CancelOrderItemSelf => {
            if ctx.is_self && ctx.within_cancel_grace {
                Allow
            } else {
                OverrideRequired(Role::Manager)
            }
        }

        // cashier+
        CloseSession | TakePayment => allow_at(Role::Cashier),
        ApplyDiscountSmall => {
            // cashier may apply if within threshold
            match ctx.discount_pct {
                Some(p) if p <= ctx.discount_threshold_pct => allow_at(Role::Cashier),
                _ => OverrideRequired(Role::Manager),
            }
        }

        // manager+
        CancelOrderItemAny | ReturnOrderItem | TransferSession | MergeSessions | SplitSession
        | ApplyDiscountLarge | ViewLiveReports | ViewReports | EditMenu => allow_at(Role::Manager),

        // owner only
        RunEod | EditRecipes | EditStaff | EditSettings | SpotEdit => allow_at(Role::Owner),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Action::*;
    use Role::*;

    fn ctx() -> PolicyCtx {
        PolicyCtx::default()
    }

    macro_rules! assert_allow {
        ($act:expr, $role:expr) => {
            assert_eq!(
                check($act, $role, ctx()),
                Decision::Allow,
                "expected Allow for {:?} as {:?}",
                $act,
                $role
            );
        };
        ($act:expr, $role:expr, $c:expr) => {
            assert_eq!(
                check($act, $role, $c),
                Decision::Allow,
                "expected Allow for {:?} as {:?}",
                $act,
                $role
            );
        };
    }
    macro_rules! assert_override {
        ($act:expr, $role:expr, $min:expr) => {
            assert_eq!(
                check($act, $role, ctx()),
                Decision::OverrideRequired($min),
                "expected OverrideRequired({:?}) for {:?} as {:?}",
                $min,
                $act,
                $role
            );
        };
        ($act:expr, $role:expr, $min:expr, $c:expr) => {
            assert_eq!(
                check($act, $role, $c),
                Decision::OverrideRequired($min),
                "expected OverrideRequired({:?}) for {:?} as {:?} with ctx",
                $min,
                $act,
                $role
            );
        };
    }

    #[test]
    fn unrestricted_actions_allow_for_staff() {
        for a in [
            OpenSession,
            PlaceOrder,
            ListRooms,
            ListTables,
            ListProducts,
            ListActiveSessions,
            GetSession,
        ] {
            assert_allow!(a, Staff);
        }
    }

    #[test]
    fn cashier_required_actions() {
        for a in [CloseSession, TakePayment] {
            assert_override!(a, Staff, Cashier);
            assert_allow!(a, Cashier);
            assert_allow!(a, Manager);
            assert_allow!(a, Owner);
        }
    }

    #[test]
    fn manager_required_actions() {
        for a in [
            CancelOrderItemAny,
            ReturnOrderItem,
            TransferSession,
            MergeSessions,
            SplitSession,
            ApplyDiscountLarge,
            ViewLiveReports,
            ViewReports,
            EditMenu,
        ] {
            assert_override!(a, Staff, Manager);
            assert_override!(a, Cashier, Manager);
            assert_allow!(a, Manager);
            assert_allow!(a, Owner);
        }
    }

    #[test]
    fn owner_required_actions() {
        for a in [RunEod, EditRecipes, EditStaff, EditSettings, SpotEdit] {
            assert_override!(a, Manager, Owner);
            assert_allow!(a, Owner);
        }
    }

    #[test]
    fn small_discount_within_threshold_for_cashier() {
        let c = PolicyCtx {
            discount_pct: Some(10),
            discount_threshold_pct: 10,
            ..ctx()
        };
        assert_allow!(ApplyDiscountSmall, Cashier, c);
    }

    #[test]
    fn small_discount_above_threshold_requires_manager() {
        let c = PolicyCtx {
            discount_pct: Some(11),
            discount_threshold_pct: 10,
            ..ctx()
        };
        assert_eq!(
            check(ApplyDiscountSmall, Cashier, c),
            Decision::OverrideRequired(Role::Manager)
        );
    }

    #[test]
    fn cancel_self_within_grace_allowed_for_staff() {
        let c = PolicyCtx {
            is_self: true,
            within_cancel_grace: true,
            ..ctx()
        };
        assert_allow!(CancelOrderItemSelf, Staff, c);
    }

    #[test]
    fn cancel_self_outside_grace_requires_manager() {
        let c = PolicyCtx {
            is_self: true,
            within_cancel_grace: false,
            ..ctx()
        };
        assert_override!(CancelOrderItemSelf, Staff, Manager, c);
    }

    #[test]
    fn cancel_other_requires_manager_even_within_grace() {
        let c = PolicyCtx {
            is_self: false,
            within_cancel_grace: true,
            ..ctx()
        };
        assert_override!(CancelOrderItemSelf, Staff, Manager, c);
    }
}

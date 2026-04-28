/// Every protected operation, named for the policy matrix.
/// One variant per Tauri command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    // session
    OpenSession,
    CloseSession,
    TransferSession,
    MergeSessions,
    SplitSession,
    // order
    PlaceOrder,
    CancelOrderItemSelf, // own item, within grace window
    CancelOrderItemAny,  // anyone's, anytime
    ReturnOrderItem,
    // payment
    TakePayment,
    ApplyDiscountSmall, // ≤ threshold
    ApplyDiscountLarge, // > threshold
    // catalog (read)
    ListRooms,
    ListTables,
    ListProducts,
    // session (read)
    ListActiveSessions,
    GetSession,
    // reports
    ViewLiveReports,
    /// Plan F: list/get historical `daily_report` rows. Manager-and-up.
    ViewReports,
    RunEod,
    // admin
    EditMenu,
    EditRecipes,
    EditStaff,
    EditSettings,
    /// Plan F: spot CRUD (rooms + tables) — Owner-only.
    SpotEdit,
    /// UTC key rotation: GET /admin/keys (list current DEK retention) — Owner-only.
    ViewKeys,
}

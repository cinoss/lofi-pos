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
    RunEod,
    // admin
    EditMenu,
    EditRecipes,
    EditStaff,
    EditSettings,
}

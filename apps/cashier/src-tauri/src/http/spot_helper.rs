use crate::app_state::AppState;
use crate::domain::spot::SpotRef;
use crate::error::{AppError, AppResult};
use crate::store::master::{Spot, SpotKind};
use std::sync::Arc;

/// Resolve a Master `Spot` row into a `SpotRef` (snapshot for event payload).
/// For rooms, snapshots the current `billing_config`. For tables with a
/// `parent_id`, looks up the parent room to populate `room_name`.
pub fn build_spot_ref(state: &Arc<AppState>, spot: Spot) -> AppResult<SpotRef> {
    Ok(match spot.kind {
        SpotKind::Room => SpotRef::Room {
            id: spot.id,
            name: spot.name,
            billing: spot
                .billing_config
                .ok_or_else(|| AppError::Validation("room missing billing_config".into()))?,
        },
        SpotKind::Table => {
            let (room_id, room_name) = if let Some(pid) = spot.parent_id {
                match state.master.lock().unwrap().get_spot(pid)? {
                    Some(p) => (Some(p.id), Some(p.name)),
                    None => (None, None),
                }
            } else {
                (None, None)
            };
            SpotRef::Table {
                id: spot.id,
                name: spot.name,
                room_id,
                room_name,
            }
        }
    })
}

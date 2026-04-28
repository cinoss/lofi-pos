use crate::app_state::AppState;
use crate::domain::spot::SpotRef;
use crate::error::{AppError, AppResult};
use crate::store::master::{Spot, SpotKind};
use std::sync::Arc;

/// Resolve a Master `Spot` row into a `SpotRef` (snapshot for event payload).
/// For tables with a `parent_id`, looks up the parent room to populate
/// `room_name`. Same logic as the deleted `resolve_spot_ref` Tauri command;
/// extracted here so the upcoming session HTTP routes can share it.
pub fn build_spot_ref(state: &Arc<AppState>, spot: Spot) -> AppResult<SpotRef> {
    Ok(match spot.kind {
        SpotKind::Room => SpotRef::Room {
            id: spot.id,
            name: spot.name,
            hourly_rate: spot
                .hourly_rate
                .ok_or_else(|| AppError::Validation("room missing rate".into()))?,
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

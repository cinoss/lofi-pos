use crate::acl::{policy::PolicyCtx, Action};
use crate::app_state::AppState;
use crate::domain::event::{DomainEvent, OrderItemSpec, RecipeIngredientSnapshot, Route};
use crate::domain::order::OrderState;
use crate::error::{AppError, AppResult};
use crate::http::auth_layer::AuthCtx;
use crate::http::error_layer::AppErrorResponse;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct RawOrderItem {
    pub product_id: i64,
    pub qty: i64,
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PlaceOrderInput {
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub session_id: String,
    pub items: Vec<RawOrderItem>,
}

#[derive(Debug, Deserialize)]
pub struct CancelOrderItemInput {
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub reason: Option<String>,
    pub is_self: bool,
    pub within_grace: bool,
}

#[derive(Debug, Deserialize)]
pub struct ReturnOrderItemInput {
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub qty: i64,
    pub reason: Option<String>,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/orders", post(place_order))
        .route("/orders/:order_id", get(get_order))
        .route("/orders/:order_id/items/:idx/cancel", post(cancel_item))
        .route("/orders/:order_id/items/:idx/return", post(return_item))
}

async fn get_order(
    State(state): State<Arc<AppState>>,
    AuthCtx(_claims): AuthCtx,
    Path(order_id): Path<String>,
) -> Result<Json<OrderState>, AppErrorResponse> {
    let s = state.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<OrderState, AppError> {
        s.commands.load_order(&order_id)?.ok_or(AppError::NotFound)
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok(Json(r))
}

fn parse_route(s: &str) -> AppResult<Route> {
    match s {
        "kitchen" => Ok(Route::Kitchen),
        "bar" => Ok(Route::Bar),
        "none" => Ok(Route::None),
        other => Err(AppError::Validation(format!("bad route: {other}"))),
    }
}

async fn place_order(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Json(input): Json<PlaceOrderInput>,
) -> Result<Json<OrderState>, AppErrorResponse> {
    let s = state.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<OrderState, AppError> {
        let order_id = Uuid::new_v4().to_string();

        let spec_items: Vec<OrderItemSpec> = {
            let master = s.master.lock().unwrap();
            let mut out = Vec::with_capacity(input.items.len());
            for raw in &input.items {
                let p = master.get_product(raw.product_id)?.ok_or_else(|| {
                    AppError::Validation(format!("product {} not found", raw.product_id))
                })?;
                let recipe = master.get_recipe(p.id)?;
                let recipe_snapshot: Vec<RecipeIngredientSnapshot> = recipe
                    .into_iter()
                    .map(|ing| RecipeIngredientSnapshot {
                        ingredient_id: ing.ingredient_id,
                        ingredient_name: ing.ingredient_name,
                        qty: ing.qty,
                        unit: ing.unit,
                    })
                    .collect();
                let route = parse_route(&p.route)?;
                out.push(OrderItemSpec {
                    product_id: p.id,
                    product_name: p.name,
                    qty: raw.qty,
                    unit_price: p.price,
                    note: raw.note.clone(),
                    route,
                    recipe_snapshot,
                });
            }
            drop(master);
            out
        };

        let event = DomainEvent::OrderPlaced {
            session_id: input.session_id.clone(),
            order_id: order_id.clone(),
            items: spec_items,
        };
        let (proj, _) = s.commands.execute(
            &claims,
            Action::PlaceOrder,
            PolicyCtx::default(),
            &input.idempotency_key,
            "place_order",
            &order_id,
            event,
            input.override_pin.as_deref(),
            |c| c.load_order(&order_id)?.ok_or(AppError::NotFound),
        )?;
        Ok(proj)
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok(Json(r))
}

async fn cancel_item(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Path((order_id, idx)): Path<(String, usize)>,
    Json(input): Json<CancelOrderItemInput>,
) -> Result<Json<OrderState>, AppErrorResponse> {
    let s = state.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<OrderState, AppError> {
        let action = if input.is_self && input.within_grace {
            Action::CancelOrderItemSelf
        } else {
            Action::CancelOrderItemAny
        };
        let ctx = PolicyCtx {
            is_self: input.is_self,
            within_cancel_grace: input.within_grace,
            ..PolicyCtx::default()
        };
        let event = DomainEvent::OrderItemCancelled {
            order_id: order_id.clone(),
            item_index: idx,
            reason: input.reason,
        };
        let (proj, _) = s.commands.execute(
            &claims,
            action,
            ctx,
            &input.idempotency_key,
            "cancel_order_item",
            &order_id,
            event,
            input.override_pin.as_deref(),
            |c| c.load_order(&order_id)?.ok_or(AppError::NotFound),
        )?;
        Ok(proj)
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok(Json(r))
}

async fn return_item(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Path((order_id, idx)): Path<(String, usize)>,
    Json(input): Json<ReturnOrderItemInput>,
) -> Result<Json<OrderState>, AppErrorResponse> {
    let s = state.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<OrderState, AppError> {
        let event = DomainEvent::OrderItemReturned {
            order_id: order_id.clone(),
            item_index: idx,
            qty: input.qty,
            reason: input.reason,
        };
        let (proj, _) = s.commands.execute(
            &claims,
            Action::ReturnOrderItem,
            PolicyCtx::default(),
            &input.idempotency_key,
            "return_order_item",
            &order_id,
            event,
            input.override_pin.as_deref(),
            |c| c.load_order(&order_id)?.ok_or(AppError::NotFound),
        )?;
        Ok(proj)
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok(Json(r))
}

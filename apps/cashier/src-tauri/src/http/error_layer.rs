use crate::error::AppError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Transparent newtype that lets axum handlers return `AppError` via `?`
/// while picking the correct HTTP status code on conversion. Body matches
/// the `AppError` Serialize contract from Plan C — `{ code, message? }`.
pub struct AppErrorResponse(pub AppError);

impl From<AppError> for AppErrorResponse {
    fn from(e: AppError) -> Self {
        Self(e)
    }
}

impl IntoResponse for AppErrorResponse {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            AppError::Validation(_) => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::OverrideRequired(_) => StatusCode::FORBIDDEN,
            AppError::Db(_)
            | AppError::Crypto(_)
            | AppError::Keychain(_)
            | AppError::Io(_)
            | AppError::Config(_)
            | AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = serde_json::to_value(&self.0).unwrap_or_else(|_| json!({"code":"internal"}));
        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acl::role::Role;
    use axum::body::to_bytes;

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn validation_maps_to_400() {
        let resp = AppErrorResponse(AppError::Validation("bad".into())).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let v = body_json(resp).await;
        assert_eq!(v["code"], "validation");
        assert_eq!(v["message"], "bad");
    }

    #[tokio::test]
    async fn unauthorized_maps_to_401() {
        let resp = AppErrorResponse(AppError::Unauthorized).into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let v = body_json(resp).await;
        assert_eq!(v["code"], "unauthorized");
    }

    #[tokio::test]
    async fn override_required_maps_to_403_with_role_in_message() {
        let resp = AppErrorResponse(AppError::OverrideRequired(Role::Manager)).into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let v = body_json(resp).await;
        assert_eq!(v["code"], "override_required");
        assert_eq!(v["message"], "manager");
    }

    #[tokio::test]
    async fn internal_maps_to_500() {
        let resp = AppErrorResponse(AppError::Internal("boom".into())).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let v = body_json(resp).await;
        assert_eq!(v["code"], "internal");
        assert_eq!(v["message"], "boom");
    }
}

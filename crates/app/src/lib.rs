//! API administrativa limitada, autenticada e com falha fechada no modo real.

use axum::{
    Json, Router,
    body::Body,
    error_handling::HandleErrorLayer,
    extract::{DefaultBodyLimit, Request, State},
    http::{StatusCode, header::AUTHORIZATION},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, put},
};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use stories_config::Secret;
use stories_store::Store;
use subtle::ConstantTimeEq;
use tower::{ServiceBuilder, limit::ConcurrencyLimitLayer};

pub mod collection;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub admin_token: Secret,
    pub dry_run: bool,
    pub collection_enabled: bool,
}

pub fn router(state: AppState) -> Router {
    let state = Arc::new(state);
    let admin = Router::new()
        .route("/status", get(status))
        .route("/pause", put(pause))
        .route_layer(middleware::from_fn_with_state(state.clone(), authorize));
    Router::new()
        .route(
            "/health/live",
            get(|| async { Json(serde_json::json!({"status": "alive"})) }),
        )
        .route("/health/ready", get(ready))
        .nest("/admin", admin)
        .layer(DefaultBodyLimit::max(1024))
        .layer(ConcurrencyLimitLayer::new(32))
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(|_: tower::BoxError| async {
                    StatusCode::REQUEST_TIMEOUT
                }))
                .timeout(Duration::from_secs(5)),
        )
        .with_state(state)
}

async fn authorize(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let supplied = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let valid = supplied.is_some_and(|token| {
        bool::from(
            token
                .as_bytes()
                .ct_eq(state.admin_token.expose().as_bytes()),
        )
    });
    if !valid {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
}

#[derive(Serialize)]
struct Readiness {
    status: &'static str,
    scope: &'static str,
    dry_run: bool,
}

async fn ready(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let healthy = state.store.health().await.is_ok() && state.dry_run;
    let code = if healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        code,
        Json(Readiness {
            status: if healthy { "ready" } else { "not_ready" },
            scope: if state.collection_enabled {
                "collection"
            } else {
                "foundation"
            },
            dry_run: state.dry_run,
        }),
    )
}

async fn status(State(state): State<Arc<AppState>>) -> Response {
    match state.store.paused().await {
        Ok(paused) => Json(serde_json::json!({
            "paused": paused, "dry_run": state.dry_run, "phase": 2,
            "collection_enabled":state.collection_enabled,"clustering_mode":"hash_entities",
            "pipeline_available": false,
        }))
        .into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PauseRequest {
    paused: bool,
}

async fn pause(State(state): State<Arc<AppState>>, Json(payload): Json<PauseRequest>) -> Response {
    match state.store.set_paused(payload.paused).await {
        Ok(()) => {
            tracing::info!(paused = payload.paused, "pausa global alterada");
            Json(serde_json::json!({"paused": payload.paused})).into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

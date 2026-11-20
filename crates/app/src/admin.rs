//! Administração autenticada: regras persistentes e associação explícita de mídia.
use crate::AppState;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::{Arc, atomic::Ordering};
use stories_core::policy::{ResolutionRequest, RuleChange};
use stories_publisher::graph::ContainerStatus;
use stories_store::{ResolutionWitness, StoreError};

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Page {
    after_id: i32,
}
fn failure(error: StoreError) -> Response {
    match error {
        StoreError::InvalidOptions | StoreError::InvalidJob => StatusCode::BAD_REQUEST,
        StoreError::Conflict | StoreError::LostLease => StatusCode::CONFLICT,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
    .into_response()
}
pub(crate) async fn rules(
    State(state): State<Arc<AppState>>,
    Query(page): Query<Page>,
) -> Response {
    match state.store.rules(page.after_id).await {
        Ok(page) => Json(page).into_response(),
        Err(e) => failure(e),
    }
}
pub(crate) async fn change_rule(
    State(state): State<Arc<AppState>>,
    Json(change): Json<RuleChange>,
) -> Response {
    match state.store.change_rule(&change).await {
        Ok(()) => Json(serde_json::json!({"changed":true})).into_response(),
        Err(e) => failure(e),
    }
}
pub(crate) async fn resolve(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<ResolutionRequest>,
) -> Response {
    if id <= 0 || request.validate().is_err() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if state.dry_run {
        return StatusCode::CONFLICT.into_response();
    }
    match state.store.resolution_receipt(id, &request).await {
        Ok(Some(result)) => return Json(result).into_response(),
        Ok(None) => {}
        Err(e) => return failure(e),
    }
    match state.store.paused().await {
        Ok(true) => {}
        Ok(false) => return StatusCode::CONFLICT.into_response(),
        Err(e) => return failure(e),
    }
    if !state.publishing_ready.load(Ordering::Relaxed) || !state.token_state.allowed().await {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let Some(graph) = state.publisher_admin.graph().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let publication = match state.store.publication(id).await {
        Ok(p) => p,
        Err(e) => return failure(e),
    };
    if publication.dry_run
        || publication.account_key.as_deref() != Some(graph.account_key().as_str())
        || publication.container_id.as_deref() != Some(request.expected_container_id.as_str())
    {
        return StatusCode::CONFLICT.into_response();
    }
    // As provas são obtidas por leitura. A associação exata é atestada pelo operador.
    let proof = tokio::try_join!(
        crate::observability::instagram(&state.store, graph.status(&request.expected_container_id)),
        crate::observability::instagram(&state.store, graph.stories())
    );
    let (status, stories) = match proof {
        Ok(proof) => proof,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let witness = ResolutionWitness {
        account: graph.account_key(),
        container_id: request.expected_container_id.clone(),
        media_id: request.media_id.clone(),
        observed_at: chrono::Utc::now(),
        container_published: status == ContainerStatus::Published,
        story_visible: stories.contains(&request.media_id),
    };
    match state.store.resolve_publication(id, &request, witness).await {
        Ok(result) => Json(result).into_response(),
        Err(e) => failure(e),
    }
}

#[cfg(test)]
mod tests;

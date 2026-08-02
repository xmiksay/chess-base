//! Integration tests for the per-user LLM provider registry HTTP surface
//! (issue #20, per-user since #198) and for the assistant removal itself: the
//! old hand-rolled assistant session routes must be gone (404 via the API
//! fallback), while `/api/assistant/providers` keeps serving the SPA.
//!
//! The HTTP harness runs local mode (implicit admin); non-admin ownership
//! rules are covered at the service level (`ProviderService` takes any
//! `CurrentUser`), same DB, same code path as the routes.

use chess_base::ai::providers::{ProviderInput, ProviderService, ProviderStoreError};
use chess_base::server::CurrentUser;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sea_orm::DatabaseConnection;
use serde_json::{json, Value};
use tower::ServiceExt;

use chess_base::db::{connect, DbConfig};
use chess_base::server::{build_router, AppState, Mode};

/// Send a request against a router over `db` (local mode = implicit admin, no
/// live provider). The same `db` can be reused across calls so persisted state
/// (e.g. an upserted provider) is visible to a later request.
async fn send(
    db: &DatabaseConnection,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let app = build_router(AppState {
        db: db.clone(),
        mode: Mode::Local,
        engine_service: None,
        provider_store: None,
        agent: Default::default(),
    });
    let builder = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(b) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = app.oneshot(request).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn mem_db() -> DatabaseConnection {
    connect(&DbConfig::in_memory()).await.unwrap()
}

#[tokio::test]
async fn removed_assistant_session_routes_are_gone() {
    let db = mem_db().await;
    for (method, uri, body) in [
        (
            "POST",
            "/api/assistant/sessions",
            Some(json!({"title":"x"})),
        ),
        ("GET", "/api/assistant/sessions", None),
        ("GET", "/api/assistant/sessions/1", None),
        (
            "POST",
            "/api/assistant/sessions/1/messages",
            Some(json!({"text":"hi"})),
        ),
        (
            "POST",
            "/api/assistant/sessions/1/respond",
            Some(json!({"decisions":{}})),
        ),
    ] {
        let (status, _) = send(&db, method, uri, body).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{method} {uri} should 404 after the assistant removal (#198)"
        );
    }
}

#[tokio::test]
async fn provider_registry_upserts_and_never_returns_the_key() {
    let db = mem_db().await;

    // Upsert a global row (local mode is the implicit admin).
    let (status, info) = send(
        &db,
        "POST",
        "/api/assistant/providers",
        Some(json!({
            "name": "anthropic",
            "wire": "anthropic",
            "model": "claude-sonnet-4-6",
            "api_key": "sk-super-secret",
            "is_default": true,
            "is_global": true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["name"], "anthropic");
    assert_eq!(info["wire"], "anthropic");
    assert_eq!(info["is_default"], true);
    assert_eq!(info["is_global"], true);
    assert_eq!(info["has_key"], true);
    assert_eq!(info["base_url"], Value::Null);
    assert!(info.get("api_key").is_none(), "upsert echoed the key back");

    // The list reflects the upserted row but omits its key.
    let (status, list) = send(&db, "GET", "/api/assistant/providers", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().map(Vec::len), Some(1));
    assert_eq!(list[0]["has_key"], true);
    assert_eq!(list[0]["is_global"], true);
    let text = serde_json::to_string(&list).unwrap();
    assert!(
        !text.contains("super-secret"),
        "the api key leaked into the provider list"
    );
}

#[tokio::test]
async fn non_admin_writes_own_rows_but_not_global_ones() {
    let db = mem_db().await;
    let svc = ProviderService::new(db.clone());
    let alice = CurrentUser {
        id: "alice".to_string(),
        is_admin: false,
    };
    let input = |is_global| ProviderInput {
        name: "zai".to_string(),
        wire: "openai".to_string(),
        model: "glm-5".to_string(),
        base_url: Some("https://api.z.ai/v1".to_string()),
        api_key: Some("sk-alice".to_string()),
        is_default: false,
        is_global,
    };

    let own = svc.upsert(&alice, input(false)).await.expect("own row");
    assert!(!own.is_global);
    assert_eq!(own.wire, "openai");

    assert!(matches!(
        svc.upsert(&alice, input(true)).await,
        Err(ProviderStoreError::Forbidden)
    ));
}

#[tokio::test]
async fn admin_global_row_shows_in_another_users_list() {
    let db = mem_db().await;

    // The implicit local admin creates a global row over HTTP.
    let (status, _) = send(
        &db,
        "POST",
        "/api/assistant/providers",
        Some(json!({
            "name": "anthropic",
            "model": "claude-sonnet-4-6",
            "api_key": "sk-super-secret",
            "is_global": true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let bob = CurrentUser {
        id: "bob".to_string(),
        is_admin: false,
    };
    let listed = ProviderService::new(db.clone())
        .list(&bob)
        .await
        .expect("list as bob");
    assert_eq!(listed.len(), 1);
    assert!(listed[0].is_global);
    let json = serde_json::to_string(&listed).unwrap();
    assert!(
        !json.contains("sk-super-secret"),
        "key leaked to another user"
    );
}

#[tokio::test]
async fn default_resolution_prefers_the_users_own_row() {
    let db = mem_db().await;
    let svc = ProviderService::new(db.clone());
    let admin = CurrentUser::local_admin();
    let alice = CurrentUser {
        id: "alice".to_string(),
        is_admin: false,
    };
    let input = |name: &str, model: &str, is_global| ProviderInput {
        name: name.to_string(),
        wire: "anthropic".to_string(),
        model: model.to_string(),
        base_url: None,
        api_key: Some("k".to_string()),
        is_default: true,
        is_global,
    };

    assert_eq!(svc.resolve_default_for(&alice).await.expect("none"), None);

    svc.upsert(&admin, input("anthropic", "m-global", true))
        .await
        .expect("global default");
    assert_eq!(
        svc.resolve_default_for(&alice).await.expect("global"),
        Some(("anthropic".to_string(), "m-global".to_string()))
    );

    svc.upsert(&alice, input("zai", "m-own", false))
        .await
        .expect("own default");
    assert_eq!(
        svc.resolve_default_for(&alice).await.expect("own"),
        Some(("zai".to_string(), "m-own".to_string()))
    );
}

#[tokio::test]
async fn deleting_an_unknown_provider_is_not_found() {
    let (status, _) = send(
        &mem_db().await,
        "DELETE",
        "/api/assistant/providers/999",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

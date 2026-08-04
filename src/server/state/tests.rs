//! Unit tests for [`AppState::llm_for_choice`] (issue #214): the per-request
//! provider/model choice seam behind the study-generation routes.

use super::*;
use crate::ai::agent::{AgentEngine, AgentProviderStore};
use crate::ai::providers::{ProviderInput, ProviderService};
use crate::db::config::{Backend, DbConfig};

async fn mem_db() -> DatabaseConnection {
    crate::db::connect(&DbConfig {
        backend: Backend::Sqlite {
            path: ":memory:".to_string(),
        },
    })
    .await
    .expect("connect in-memory db")
}

fn bare_state(db: DatabaseConnection) -> AppState {
    AppState {
        db,
        mode: Mode::Local,
        engine_service: None,
        provider_store: None,
        agent: Default::default(),
    }
}

fn input(name: &str, model: &str, is_default: bool) -> ProviderInput {
    ProviderInput {
        name: name.to_string(),
        wire: "anthropic".to_string(),
        model: model.to_string(),
        base_url: None,
        api_key: Some("secret".to_string()),
        is_default,
        is_global: false,
    }
}

/// The contract: an explicit `provider` without a `model` is the caller's
/// error, checked before any engine/agent state is touched.
#[tokio::test]
async fn provider_without_model_is_invalid() {
    let state = bare_state(mem_db().await);
    let user = CurrentUser::local_admin();
    match state.llm_for_choice(&user, Some("anthropic"), None) {
        Err(LlmChoiceError::Invalid(msg)) => assert!(msg.contains("model"), "{msg}"),
        Err(other) => panic!("expected Invalid, got {other:?}"),
        Ok(_) => panic!("provider without model must not resolve"),
    }
}

/// Without a running agent engine there is no provider surface: both the
/// default path and an explicit choice are `Unconfigured` (the routes' 503),
/// never a panic or a misleading 400.
#[tokio::test]
async fn without_an_agent_engine_everything_is_unconfigured() {
    let state = bare_state(mem_db().await);
    let user = CurrentUser::local_admin();
    assert!(matches!(
        state.llm_for_choice(&user, None, None),
        Err(LlmChoiceError::Unconfigured)
    ));
    assert!(matches!(
        state.llm_for_choice(&user, Some("anthropic"), Some("claude-x")),
        Err(LlmChoiceError::Unconfigured)
    ));
}

/// With the engine running, a named non-default row resolves (the whole point
/// of #214 — the default row no longer wins unconditionally) and an unknown
/// name is the caller's error, distinct from `Unconfigured`.
#[tokio::test]
async fn named_row_resolves_and_unknown_name_is_invalid() {
    let db = mem_db().await;
    let user = CurrentUser::local_admin();
    let svc = ProviderService::new(db.clone());
    svc.upsert(&user, input("anthropic", "claude-a", true))
        .await
        .expect("default row");
    svc.upsert(&user, input("zai", "glm-5", false))
        .await
        .expect("non-default row");

    let store = AgentProviderStore::new_with_env(db.clone(), None)
        .await
        .expect("provider store");
    let state = AppState {
        provider_store: Some(store),
        ..bare_state(db)
    };
    let engine = AgentEngine::start(state.clone()).await.expect("engine");
    state.agent.set(engine).ok();

    let provider = state
        .llm_for_choice(&user, Some("zai"), Some("glm-5"))
        .expect("named non-default row resolves");
    assert_eq!(provider.default_model(), "glm-5");

    match state.llm_for_choice(&user, Some("nope"), Some("m")) {
        Err(LlmChoiceError::Invalid(_)) => {}
        Err(other) => panic!("unknown provider must be Invalid, got {other:?}"),
        Ok(_) => panic!("unknown provider must not resolve"),
    }

    if let Some(engine) = state.agent() {
        engine.shutdown();
    }
}

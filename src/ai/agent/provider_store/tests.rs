//! Unit tests for [`AgentProviderStore`] over an in-memory SQLite database.

use super::*;
use crate::db::config::{Backend, DbConfig};
use sea_orm::{ActiveModelTrait, Set};

async fn mem_db() -> DatabaseConnection {
    crate::db::connect(&DbConfig {
        backend: Backend::Sqlite {
            path: ":memory:".to_string(),
        },
    })
    .await
    .expect("connect in-memory db")
}

async fn seed(
    db: &DatabaseConnection,
    owner: Option<&str>,
    name: &str,
    wire: &str,
    model: &str,
    key: &str,
    base_url: Option<&str>,
) {
    llm_providers::ActiveModel {
        name: Set(name.to_string()),
        model: Set(model.to_string()),
        api_key: Set(key.to_string()),
        is_default: Set(false),
        owner_id: Set(owner.map(str::to_string)),
        wire: Set(wire.to_string()),
        base_url: Set(base_url.map(str::to_string)),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed provider row");
}

fn names(ctx: &UserProviderContext) -> Vec<String> {
    ctx.catalog
        .providers
        .iter()
        .map(|p| p.name.clone())
        .collect()
}

#[tokio::test]
async fn two_users_get_their_own_catalogs() {
    let db = mem_db().await;
    seed(
        &db,
        Some("alice"),
        "zai",
        "openai",
        "glm-5",
        "alice-key",
        Some("https://api.z.ai/v1"),
    )
    .await;
    seed(
        &db,
        Some("bob"),
        "anthropic",
        "anthropic",
        "claude-x",
        "bob-key",
        None,
    )
    .await;
    let store = AgentProviderStore::new_with_env(db, None)
        .await
        .expect("build store");

    let alice = store.context(&UserId::new("alice")).expect("alice context");
    assert_eq!(names(&alice), vec!["zai"]);
    assert_eq!(alice.catalog.providers[0].default_model, "glm-5");
    assert_eq!(
        alice.catalog.providers[0].base_url.as_deref(),
        Some("https://api.z.ai/v1")
    );

    let bob = store.context(&UserId::new("bob")).expect("bob context");
    assert_eq!(names(&bob), vec!["anthropic"]);
    assert_eq!(bob.catalog.providers[0].default_model, "claude-x");
    assert_eq!(bob.catalog.providers[0].wire, Wire::Anthropic);
}

#[tokio::test]
async fn user_without_rows_falls_back_to_house() {
    let db = mem_db().await;
    seed(
        &db,
        None,
        "anthropic",
        "anthropic",
        "claude-x",
        "g-key",
        None,
    )
    .await;
    let store = AgentProviderStore::new_with_env(db, None)
        .await
        .expect("build store");

    let carol = store
        .context(&UserId::new("carol"))
        .expect("house fallback");
    assert_eq!(names(&carol), vec!["anthropic"]);
    assert_eq!(carol.catalog.providers[0].default_model, "claude-x");
}

#[tokio::test]
async fn house_comes_from_global_rows_then_env() {
    // Global rows win.
    let db = mem_db().await;
    seed(&db, None, "openai", "openai", "gpt-x", "g-key", None).await;
    let store = AgentProviderStore::new_with_env(db, Some("sk-env".to_string()))
        .await
        .expect("build store");
    let house = store.context(&UserId::new(HOUSE_USER)).expect("house");
    assert_eq!(names(&house), vec!["openai"]);

    // No global rows: the env key synthesizes a builtin-backed anthropic entry.
    let db = mem_db().await;
    let store = AgentProviderStore::new_with_env(db, Some("sk-env".to_string()))
        .await
        .expect("build store");
    let house = store.context(&UserId::new(HOUSE_USER)).expect("env house");
    assert_eq!(names(&house), vec!["anthropic"]);
    let builtin_default = Catalog::builtin()
        .provider("anthropic")
        .expect("builtin anthropic")
        .default_model
        .clone();
    assert_eq!(house.catalog.providers[0].default_model, builtin_default);
    assert!(
        !house.catalog.providers[0].models.is_empty(),
        "env fallback should carry the builtin anthropic models"
    );

    // Neither global rows nor env key: no house context at all.
    let db = mem_db().await;
    let store = AgentProviderStore::new_with_env(db, None)
        .await
        .expect("build store");
    assert!(store.context(&UserId::new(HOUSE_USER)).is_none());
    assert!(store.context(&UserId::new("nobody")).is_none());
}

#[tokio::test]
async fn invalidate_reflects_an_upsert() {
    let db = mem_db().await;
    let store = AgentProviderStore::new_with_env(db.clone(), None)
        .await
        .expect("build store");
    assert!(store.context(&UserId::new("alice")).is_none());

    seed(
        &db,
        Some("alice"),
        "anthropic",
        "anthropic",
        "claude-x",
        "a-key",
        None,
    )
    .await;
    // Not visible until invalidated — the trait read is a pure cache read.
    assert!(store.context(&UserId::new("alice")).is_none());

    store.invalidate(Some("alice")).await.expect("invalidate");
    let alice = store.context(&UserId::new("alice")).expect("alice context");
    assert_eq!(names(&alice), vec!["anthropic"]);
}

#[tokio::test]
async fn user_row_overrides_same_named_global_row() {
    let db = mem_db().await;
    seed(
        &db,
        None,
        "anthropic",
        "anthropic",
        "m-global",
        "g-key",
        None,
    )
    .await;
    seed(&db, None, "openai", "openai", "gpt-x", "g-key2", None).await;
    seed(
        &db,
        Some("alice"),
        "anthropic",
        "anthropic",
        "m-user",
        "a-key",
        None,
    )
    .await;
    let store = AgentProviderStore::new_with_env(db, None)
        .await
        .expect("build store");

    let alice = store.context(&UserId::new("alice")).expect("alice context");
    // Own anthropic row wins; the non-overridden global still composes in.
    let mut got = names(&alice);
    got.sort();
    assert_eq!(got, vec!["anthropic", "openai"]);
    let anthropic = alice.catalog.provider("anthropic").expect("anthropic");
    assert_eq!(anthropic.default_model, "m-user");
}

#[tokio::test]
async fn unknown_wire_needs_a_base_url() {
    let db = mem_db().await;
    // Unknown wire without an endpoint: unusable, skipped.
    seed(
        &db,
        Some("alice"),
        "mystery",
        "carrier-pigeon",
        "m",
        "k",
        None,
    )
    .await;
    // Unknown wire with an endpoint: assumed OpenAI-compatible.
    seed(
        &db,
        Some("alice"),
        "proxy",
        "carrier-pigeon",
        "m2",
        "k2",
        Some("https://proxy.example/v1"),
    )
    .await;
    let store = AgentProviderStore::new_with_env(db, None)
        .await
        .expect("build store");

    let alice = store.context(&UserId::new("alice")).expect("alice context");
    assert_eq!(names(&alice), vec!["proxy"]);
    assert_eq!(alice.catalog.providers[0].wire, Wire::Openai);
}

#[tokio::test]
async fn resolver_falls_back_to_the_house_user() {
    let db = mem_db().await;
    seed(
        &db,
        None,
        "anthropic",
        "anthropic",
        "claude-x",
        "g-key",
        None,
    )
    .await;
    let store = AgentProviderStore::new_with_env(db, None)
        .await
        .expect("build store");
    let resolver = store.build_resolver(HttpClient::new().expect("http client"));

    // No session user: resolves against the house context instead of erroring.
    let resolved = resolver(None, "anthropic", "claude-x").expect("house resolution");
    assert_eq!(resolved.provider, "anthropic");
    assert_eq!(resolved.model, "claude-x");

    // Unknown provider still fails loudly.
    assert!(resolver(None, "nope", "m").is_err());
}

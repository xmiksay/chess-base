//! Unit tests for the ownership-aware [`ProviderService`].

use super::*;
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

fn plain(id: &str) -> CurrentUser {
    CurrentUser {
        id: id.to_string(),
        is_admin: false,
    }
}

fn input(name: &str, model: &str) -> ProviderInput {
    ProviderInput {
        name: name.to_string(),
        wire: "anthropic".to_string(),
        model: model.to_string(),
        base_url: None,
        api_key: Some("secret".to_string()),
        is_default: false,
        is_global: false,
    }
}

#[tokio::test]
async fn plain_user_writes_own_rows_but_never_global_ones() {
    let svc = ProviderService::new(mem_db().await);
    let alice = plain("alice");

    let info = svc
        .upsert(&alice, input("anthropic", "m1"))
        .await
        .expect("own upsert");
    assert!(!info.is_global);
    assert!(info.has_key);

    let global = ProviderInput {
        is_global: true,
        ..input("anthropic", "m1")
    };
    assert!(matches!(
        svc.upsert(&alice, global).await,
        Err(ProviderStoreError::Forbidden)
    ));
}

#[tokio::test]
async fn global_rows_are_visible_to_every_user_and_keys_never_serialize() {
    let svc = ProviderService::new(mem_db().await);
    let admin = CurrentUser::local_admin();
    let global = ProviderInput {
        is_global: true,
        ..input("anthropic", "claude-x")
    };
    let info = svc.upsert(&admin, global).await.expect("global upsert");
    assert!(info.is_global);

    let listed = svc.list(&plain("bob")).await.expect("list as bob");
    assert_eq!(listed.len(), 1);
    assert!(listed[0].is_global);
    assert!(listed[0].has_key);
    let json = serde_json::to_string(&listed).expect("serialize");
    assert!(!json.contains("secret"), "api key leaked into list output");
}

#[tokio::test]
async fn users_only_see_their_own_and_global_rows() {
    let svc = ProviderService::new(mem_db().await);
    svc.upsert(&plain("alice"), input("zai", "glm"))
        .await
        .expect("alice upsert");
    svc.upsert(&plain("bob"), input("openai", "gpt"))
        .await
        .expect("bob upsert");

    let alice_rows = svc.list(&plain("alice")).await.expect("list");
    assert_eq!(alice_rows.len(), 1);
    assert_eq!(alice_rows[0].name, "zai");
}

#[tokio::test]
async fn default_is_per_owner_and_update_keeps_the_key() {
    let svc = ProviderService::new(mem_db().await);
    let alice = plain("alice");
    for name in ["one", "two"] {
        svc.upsert(
            &alice,
            ProviderInput {
                is_default: true,
                ..input(name, "m")
            },
        )
        .await
        .expect("upsert");
    }
    let rows = svc.list(&alice).await.expect("list");
    let defaults: Vec<_> = rows.iter().filter(|p| p.is_default).collect();
    assert_eq!(
        defaults.len(),
        1,
        "only the latest default should remain set"
    );
    assert_eq!(defaults[0].name, "two");

    // Updating without a key keeps the stored one.
    let updated = svc
        .upsert(
            &alice,
            ProviderInput {
                api_key: None,
                ..input("two", "m2")
            },
        )
        .await
        .expect("keyless update");
    assert!(updated.has_key, "api_key: None must keep the existing key");
    assert_eq!(updated.model, "m2");
}

#[tokio::test]
async fn resolve_default_prefers_the_users_row_over_the_global_one() {
    let svc = ProviderService::new(mem_db().await);
    let admin = CurrentUser::local_admin();
    let alice = plain("alice");

    assert_eq!(svc.resolve_default_for(&alice).await.expect("none"), None);

    svc.upsert(
        &admin,
        ProviderInput {
            is_global: true,
            is_default: true,
            ..input("anthropic", "m-global")
        },
    )
    .await
    .expect("global default");
    assert_eq!(
        svc.resolve_default_for(&alice).await.expect("global"),
        Some(("anthropic".to_string(), "m-global".to_string()))
    );

    svc.upsert(
        &alice,
        ProviderInput {
            is_default: true,
            ..input("zai", "m-own")
        },
    )
    .await
    .expect("own default");
    assert_eq!(
        svc.resolve_default_for(&alice).await.expect("own"),
        Some(("zai".to_string(), "m-own".to_string()))
    );
}

#[tokio::test]
async fn delete_is_owner_or_admin_for_global() {
    let svc = ProviderService::new(mem_db().await);
    let admin = CurrentUser::local_admin();
    let alice = plain("alice");

    let own = svc.upsert(&alice, input("zai", "m")).await.expect("own");
    let global = svc
        .upsert(
            &admin,
            ProviderInput {
                is_global: true,
                ..input("anthropic", "m")
            },
        )
        .await
        .expect("global");

    // A plain user cannot delete a global row.
    assert!(matches!(
        svc.delete(&alice, global.id).await,
        Err(ProviderStoreError::Forbidden)
    ));
    // But deletes their own; the admin deletes the global row.
    svc.delete(&alice, own.id).await.expect("delete own");
    svc.delete(&admin, global.id).await.expect("delete global");
    assert!(matches!(
        svc.delete(&admin, global.id).await,
        Err(ProviderStoreError::NotFound)
    ));
}

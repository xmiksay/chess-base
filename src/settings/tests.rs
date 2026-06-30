//! Service-level tests over an in-memory SQLite DB: defaults when unset, the
//! round trip + per-user scoping, theme/default-database validation, blank-field
//! normalization and persistence across a rebuilt service (same DB).

use super::*;
use crate::databases::DatabaseService;
use crate::db::{connect, DbConfig};
use sea_orm::DatabaseConnection;

fn user(id: &str) -> CurrentUser {
    CurrentUser {
        id: id.to_string(),
        is_admin: false,
    }
}

async fn conn() -> DatabaseConnection {
    connect(&DbConfig::in_memory()).await.unwrap()
}

#[tokio::test]
async fn get_returns_defaults_when_unset() {
    let svc = SettingsService::new(conn().await);
    let got = svc.get(&user("alice")).await.unwrap();
    assert_eq!(got, UserSettings::default());
    assert!(got.theme.is_none());
}

#[tokio::test]
async fn set_then_get_round_trips_and_returns_canonical() {
    let svc = SettingsService::new(conn().await);
    let alice = user("alice");

    let saved = svc
        .set(
            &alice,
            UserSettings {
                theme: Some("dark".into()),
                board_theme: Some("  blue  ".into()),
                piece_set: Some("cburnett".into()),
                default_database_id: None,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Returned value is normalized (board theme trimmed).
    assert_eq!(saved.board_theme.as_deref(), Some("blue"));

    let got = svc.get(&alice).await.unwrap();
    assert_eq!(got.theme.as_deref(), Some("dark"));
    assert_eq!(got.board_theme.as_deref(), Some("blue"));
    assert_eq!(got.piece_set.as_deref(), Some("cburnett"));
}

#[tokio::test]
async fn overlay_layer_flags_round_trip() {
    let svc = SettingsService::new(conn().await);
    let alice = user("alice");

    // Unset by default — the frontend supplies the layer defaults.
    let got = svc.get(&alice).await.unwrap();
    assert_eq!(got.show_plans, None);
    assert_eq!(got.show_threats, None);
    assert_eq!(got.show_master_moves, None);

    let saved = svc
        .set(
            &alice,
            UserSettings {
                show_plans: Some(false),
                show_threats: Some(true),
                show_master_moves: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(saved.show_plans, Some(false));

    let got = svc.get(&alice).await.unwrap();
    assert_eq!(got.show_plans, Some(false));
    assert_eq!(got.show_threats, Some(true));
    assert_eq!(got.show_master_moves, Some(true));
}

#[tokio::test]
async fn settings_are_scoped_per_user() {
    let db = conn().await;
    let svc = SettingsService::new(db);
    let alice = user("alice");
    let bob = user("bob");

    svc.set(
        &alice,
        UserSettings {
            theme: Some("dark".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Bob's settings are untouched by Alice's write.
    assert_eq!(svc.get(&bob).await.unwrap(), UserSettings::default());
    assert_eq!(
        svc.get(&alice).await.unwrap().theme.as_deref(),
        Some("dark")
    );
}

#[tokio::test]
async fn blank_strings_normalize_to_none() {
    let svc = SettingsService::new(conn().await);
    let alice = user("alice");

    let saved = svc
        .set(
            &alice,
            UserSettings {
                theme: None,
                board_theme: Some("   ".into()),
                piece_set: Some("".into()),
                default_database_id: None,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert!(saved.board_theme.is_none());
    assert!(saved.piece_set.is_none());
}

#[tokio::test]
async fn rejects_unknown_theme() {
    let svc = SettingsService::new(conn().await);
    let err = svc
        .set(
            &user("alice"),
            UserSettings {
                theme: Some("neon".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, SettingsError::InvalidInput(_)));
}

#[tokio::test]
async fn default_database_must_be_visible() {
    let db = conn().await;
    let dbs = DatabaseService::new(db.clone());
    let svc = SettingsService::new(db);
    let alice = user("alice");
    let bob = user("bob");

    let mine = dbs.create(&alice, "Mine", "own", false).await.unwrap();
    let theirs = dbs.create(&bob, "Theirs", "own", false).await.unwrap();

    // Alice may point at her own database.
    let saved = svc
        .set(
            &alice,
            UserSettings {
                default_database_id: Some(mine.id),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(saved.default_database_id, Some(mine.id));

    // ...but not at Bob's, nor at a non-existent id.
    assert!(matches!(
        svc.set(
            &alice,
            UserSettings {
                default_database_id: Some(theirs.id),
                ..Default::default()
            },
        )
        .await
        .unwrap_err(),
        SettingsError::InvalidInput(_)
    ));
    assert!(matches!(
        svc.set(
            &alice,
            UserSettings {
                default_database_id: Some(9999),
                ..Default::default()
            },
        )
        .await
        .unwrap_err(),
        SettingsError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn persists_across_a_rebuilt_service() {
    let db = conn().await;
    let alice = user("alice");

    SettingsService::new(db.clone())
        .set(
            &alice,
            UserSettings {
                theme: Some("light".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // A fresh service over the same connection still sees the persisted value.
    let got = SettingsService::new(db).get(&alice).await.unwrap();
    assert_eq!(got.theme.as_deref(), Some("light"));
}

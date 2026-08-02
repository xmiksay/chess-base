//! Engine bootstrap tests, end-to-end over `EchoLlm`.
//!
//! The fixture's provider store is built hermetically with **no** rows and
//! **no** env key, so the `build` profile's [`DEFAULT_PIN`] pin fails to
//! resolve at session start (a logged warning) and every turn runs on the
//! `EngineConfig` default `EchoLlm` — deterministic, no network. The gated
//! ToolRequest/approval park is *not* exercised here: `EchoLlm` never calls
//! tools (the policy grant flow is covered by `policy::tests`, and steps 5/6
//! drive a scripted tool-calling turn over the relay).

use super::*;
use crate::ai::agent::persistence::load_records;
use crate::db::config::DbConfig;
use crate::db::entities::llm_providers;
use crate::server::config::Mode;
use crate::server::identity::CurrentUser;
use entanglement_core::InMsg;
use entanglement_provider::UserId;
use entanglement_runtime::session_store::{integrity_gap, pair_records, LogPayload};
use sea_orm::{ActiveModelTrait, Set};
use std::time::Duration;

async fn dummy_app() -> AppState {
    let db = crate::db::connect(&DbConfig::in_memory())
        .await
        .expect("connect in-memory db");
    let store = AgentProviderStore::new_with_env(db.clone(), None)
        .await
        .expect("build provider store");
    AppState {
        db,
        mode: Mode::Local,
        engine_service: None,
        provider_store: Some(store),
        agent: Default::default(),
    }
}

fn spawn_msg(root: &SessionId, prompt: &str) -> InMsg {
    InMsg::Spawn {
        session: root.clone(),
        parent: None,
        predecessor: None,
        agent: AGENT_PROFILE.to_string(),
        prompt: prompt.to_string(),
        user: Some(UserId::new("local")),
    }
}

/// Drain `sub` until `root`'s `Done`, collecting its streamed text.
async fn collect_turn(
    sub: &mut tokio::sync::broadcast::Receiver<OutEvent>,
    root: &SessionId,
) -> String {
    let mut text = String::new();
    let deadline = tokio::time::Duration::from_secs(10);
    loop {
        let ev = tokio::time::timeout(deadline, sub.recv())
            .await
            .expect("turn timed out")
            .expect("event stream open");
        if ev.session() != Some(root) {
            continue;
        }
        match ev {
            OutEvent::TextDelta { text: t, .. } => text.push_str(&t),
            OutEvent::Done { .. } => return text,
            OutEvent::Error { message, .. } => panic!("engine error: {message}"),
            _ => {}
        }
    }
}

/// Poll until `root`'s persisted log contains a `Done` event (the sink's
/// writer task is asynchronous).
async fn wait_for_done_record(db: &sea_orm::DatabaseConnection, root: &str) {
    for _ in 0..250 {
        let records = load_records(db, root).await.expect("load records");
        let done = records
            .iter()
            .any(|r| matches!(&r.payload, LogPayload::Out(OutEvent::Done { .. })));
        if done {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("Done event never persisted for {root}");
}

#[tokio::test]
async fn spawned_session_streams_a_turn_and_the_sink_persists_it_in_order() {
    let app = dummy_app().await;
    let engine = AgentEngine::start(app.clone()).await.expect("engine start");
    let mut sub = engine.holly.subscribe();

    let root = SessionId::new("local:e2e-turn");
    engine.users.register(root.clone(), UserId::new("local"));
    engine
        .holly
        .send(spawn_msg(&root, "hello engine"))
        .await
        .expect("send spawn");

    let text = collect_turn(&mut sub, &root).await;
    assert!(
        text.contains("hello engine"),
        "EchoLlm reply must quote the prompt, got: {text}"
    );

    wait_for_done_record(&app.db, &root.0).await;
    let records = load_records(&app.db, &root.0).await.expect("load records");
    assert!(integrity_gap(&records).is_none(), "no loss in a quiet test");
    // The synthesized spawn prompt and the streamed reply are both there.
    assert!(records
        .iter()
        .any(|r| matches!(&r.payload, LogPayload::In(InMsg::Prompt { .. }))));
    assert!(records
        .iter()
        .any(|r| matches!(&r.payload, LogPayload::Out(OutEvent::TextDelta { .. }))));
    // Strict ord order is what `load_records` sorts by; the rows carry the
    // session's own event order (SessionStarted before any TextDelta).
    let started_at = records
        .iter()
        .position(|r| matches!(&r.payload, LogPayload::Out(OutEvent::SessionStarted { .. })))
        .expect("SessionStarted persisted");
    let first_delta = records
        .iter()
        .position(|r| matches!(&r.payload, LogPayload::Out(OutEvent::TextDelta { .. })))
        .expect("TextDelta persisted");
    assert!(started_at < first_delta);

    // The index subscriber marked the root live.
    assert!(engine.sessions.is_live(&root));

    engine.shutdown();
}

#[tokio::test]
async fn a_new_engine_resumes_from_the_persisted_log_and_answers_again() {
    let app = dummy_app().await;
    let engine = AgentEngine::start(app.clone()).await.expect("engine start");
    let mut sub = engine.holly.subscribe();

    let root = SessionId::new("local:e2e-resume");
    engine.users.register(root.clone(), UserId::new("local"));
    engine
        .holly
        .send(spawn_msg(&root, "first prompt"))
        .await
        .expect("send spawn");
    collect_turn(&mut sub, &root).await;
    wait_for_done_record(&app.db, &root.0).await;
    engine.shutdown();

    // "Restart": a fresh engine core (same profiles, EchoLlm — no resolver),
    // rebuilt purely from what the sink persisted.
    let records = load_records(&app.db, &root.0).await.expect("load records");
    assert!(integrity_gap(&records).is_none());
    let holly = Holly::spawn(EngineConfig {
        profiles: agent_profiles(),
        ..EngineConfig::default()
    });
    let mut sub = holly.subscribe();
    holly
        .resume(root.clone(), pair_records(&records))
        .await
        .expect("resume");
    // Resume re-announces the session, then a second prompt round-trips.
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(10), sub.recv())
            .await
            .expect("resume timed out")
            .expect("stream open");
        if matches!(&ev, OutEvent::SessionStarted { session, .. } if session == &root) {
            break;
        }
    }
    holly
        .send(InMsg::prompt(root.clone(), "second prompt"))
        .await
        .expect("send prompt");
    let text = collect_turn(&mut sub, &root).await;
    // EchoLlm summarizes every user message it sees: a resumed context keeps
    // the first turn, so both prompts show up.
    assert!(text.contains("first prompt"), "history survived: {text}");
    assert!(
        text.contains("second prompt"),
        "new prompt answered: {text}"
    );
}

#[tokio::test]
async fn llm_for_is_none_without_an_engine_or_any_provider_rows() {
    let app = dummy_app().await;
    let admin = CurrentUser::local_admin();
    // No engine set on the state at all.
    assert!(app.llm_for(&admin).is_none());

    // Engine running, but the hermetic store has no rows and no env key.
    let engine = AgentEngine::start(app.clone()).await.expect("engine start");
    app.agent.set(engine.clone()).ok().expect("set agent once");
    assert!(app.llm_for(&admin).is_none());
    engine.shutdown();
}

#[tokio::test]
async fn llm_for_resolves_the_callers_default_provider_row() {
    let db = crate::db::connect(&DbConfig::in_memory())
        .await
        .expect("connect in-memory db");
    let admin = CurrentUser::local_admin();
    // An openai-wire row: construction needs no key and no network, so
    // resolution succeeds without ever contacting the (dead) endpoint.
    llm_providers::ActiveModel {
        name: Set("zai".to_string()),
        model: Set("glm-5".to_string()),
        api_key: Set("k".to_string()),
        is_default: Set(true),
        owner_id: Set(Some(admin.id.clone())),
        wire: Set("openai".to_string()),
        base_url: Set(Some("http://127.0.0.1:1/v1".to_string())),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed provider row");

    let store = AgentProviderStore::new_with_env(db.clone(), None)
        .await
        .expect("build provider store");
    let app = AppState {
        db,
        mode: Mode::Local,
        engine_service: None,
        provider_store: Some(store),
        agent: Default::default(),
    };
    let engine = AgentEngine::start(app.clone()).await.expect("engine start");
    app.agent.set(engine.clone()).ok().expect("set agent once");

    let provider = app.llm_for(&admin).expect("default row resolves");
    assert_eq!(provider.default_model(), "glm-5");
    assert_eq!(provider.name(), "entanglement");
    engine.shutdown();
}

#[tokio::test]
async fn start_reports_no_provider_surface_on_health_signal() {
    let app = dummy_app().await;
    let engine = AgentEngine::start(app.clone()).await.expect("engine start");
    assert!(
        !engine.providers.has_any_context(),
        "hermetic fixture has no provider surface at all"
    );
    engine.shutdown();
}

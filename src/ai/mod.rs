//! AI integration.
//!
//! [`llm`] is the provider-agnostic LLM client layer — the request/response
//! types the batch annotation pass (Epic 9) programs against. [`agent`] is the
//! embedded entanglement agent engine (#198) replacing the old hand-rolled
//! assistant; [`providers`] is the admin-managed provider registry backing it.

pub mod agent;
pub mod llm;
pub mod providers;

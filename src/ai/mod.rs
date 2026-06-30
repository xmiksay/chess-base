//! AI integration.
//!
//! [`llm`] is the provider-agnostic LLM client layer — the foundation shared by
//! the planned batch annotation pass (Epic 9) and the interactive assistant
//! (Epic 7). Both need a Claude API client; extracting it here keeps the core
//! batch pass from depending on the optional interactive work.

pub mod assistant;
pub mod llm;
pub mod providers;

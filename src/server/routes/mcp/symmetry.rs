//! The MCP/HTTP symmetry check (issue #183, ADR-0036): a hand-maintained
//! manifest asserting every non-carve-out HTTP route has a registered MCP
//! twin, so the two surfaces can't silently drift apart again the way they did
//! before this issue (16 MCP tools vs. ~45 HTTP routes).
//!
//! This is deliberately a **doc-driven manifest**, not a runtime introspection
//! of the Axum router (Axum doesn't expose a route list to walk). The
//! discipline it enforces: every PR that adds an HTTP route adds a row here in
//! the same change — either a `tool` name (checked against the live registry
//! below) or an entry in [`CARVE_OUTS`] with a one-line reason. What it does
//! **not** catch: a route added without touching this file at all — that's a
//! code-review concern, not a compile-time one.
//!
//! [`KNOWN_GAPS`] lists routes this issue's audit found but chose not to mirror
//! (pre-existing debt, out of #183's scope) — tracked so they read as a
//! decision, not an oversight.

/// One HTTP route this issue's symmetry pass considered, and the MCP tool that
/// now mirrors it.
struct RouteEntry {
    method: &'static str,
    path: &'static str,
    tool: &'static str,
}

/// Every non-carve-out route covered by this issue, mapped to its MCP tool.
/// Routes that predate #183 and already had a tool (e.g. `study_create`) are
/// included too, so this manifest is the *complete* symmetry contract going
/// forward, not just the delta.
const ROUTE_MANIFEST: &[RouteEntry] = &[
    // --- studies (studies/routes.rs) ---
    RouteEntry {
        method: "GET",
        path: "/api/studies",
        tool: "study_list",
    },
    RouteEntry {
        method: "POST",
        path: "/api/studies",
        tool: "study_create",
    },
    RouteEntry {
        method: "POST",
        path: "/api/studies/import",
        tool: "study_import_pgn",
    },
    RouteEntry {
        method: "POST",
        path: "/api/studies/merge-games",
        tool: "study_merge_games",
    },
    RouteEntry {
        method: "GET",
        path: "/api/studies/{id}",
        tool: "study_get",
    },
    RouteEntry {
        method: "PUT",
        path: "/api/studies/{id}/folder",
        tool: "study_set_folder",
    },
    RouteEntry {
        method: "GET",
        path: "/api/studies/{id}/export",
        tool: "study_export",
    },
    RouteEntry {
        method: "GET",
        path: "/api/studies/{id}/export/lichess",
        tool: "study_export",
    },
    RouteEntry {
        method: "POST",
        path: "/api/studies/{id}/analyse",
        tool: "study_analyse",
    },
    RouteEntry {
        method: "POST",
        path: "/api/studies/{id}/moves",
        tool: "study_add_move",
    },
    RouteEntry {
        method: "POST",
        path: "/api/studies/{id}/nodes/{node_id}/annotate",
        tool: "study_annotate",
    },
    RouteEntry {
        method: "PUT",
        path: "/api/studies/{id}/nodes/{node_id}/shapes",
        tool: "study_set_shapes",
    },
    RouteEntry {
        method: "POST",
        path: "/api/studies/{id}/nodes/{node_id}/promote",
        tool: "study_promote_node",
    },
    RouteEntry {
        method: "POST",
        path: "/api/studies/{id}/nodes/{node_id}/reorder",
        tool: "study_reorder_node",
    },
    // --- studies auxiliary routes ---
    RouteEntry {
        method: "POST",
        path: "/api/studies/{id}/merge-danger",
        tool: "study_merge_danger",
    },
    RouteEntry {
        method: "POST",
        path: "/api/studies/danger-map",
        tool: "danger_map",
    },
    // --- games (games/routes.rs) ---
    RouteEntry {
        method: "GET",
        path: "/api/games",
        tool: "db_list_games",
    },
    RouteEntry {
        method: "GET",
        path: "/api/games/{id}",
        tool: "db_read_game",
    },
    RouteEntry {
        method: "DELETE",
        path: "/api/games/{id}",
        tool: "game_delete",
    },
    RouteEntry {
        method: "GET",
        path: "/api/games/{id}/tree",
        tool: "game_tree",
    },
    RouteEntry {
        method: "GET",
        path: "/api/games/{id}/export",
        tool: "db_read_game",
    },
    RouteEntry {
        method: "POST",
        path: "/api/games/export",
        tool: "db_export_games",
    },
    RouteEntry {
        method: "POST",
        path: "/api/games/{id}/save-as-study",
        tool: "game_save_as_study",
    },
    RouteEntry {
        method: "GET",
        path: "/api/games/{id}/studies",
        tool: "game_studies",
    },
    // --- folders (folders/routes.rs) ---
    RouteEntry {
        method: "GET",
        path: "/api/folders",
        tool: "folder_list",
    },
    RouteEntry {
        method: "POST",
        path: "/api/folders",
        tool: "folder_create",
    },
    RouteEntry {
        method: "PATCH",
        path: "/api/folders/{id}",
        tool: "folder_update",
    },
    RouteEntry {
        method: "DELETE",
        path: "/api/folders/{id}",
        tool: "folder_delete",
    },
    // --- search (search/routes.rs) ---
    RouteEntry {
        method: "GET",
        path: "/api/search/tree",
        tool: "db_position_report",
    },
    RouteEntry {
        method: "GET",
        path: "/api/search/games",
        tool: "db_reference_games",
    },
    RouteEntry {
        method: "GET",
        path: "/api/search/headers",
        tool: "search_headers",
    },
    // --- threats (threats/routes.rs) ---
    RouteEntry {
        method: "GET",
        path: "/api/threats",
        tool: "position_threats",
    },
    // --- imports (imports/routes.rs) ---
    RouteEntry {
        method: "POST",
        path: "/api/import/pgn",
        tool: "import_pgn",
    },
    RouteEntry {
        method: "POST",
        path: "/api/import/sync",
        tool: "import_sync",
    },
];

/// Routes that stay HTTP-only by design (ADR-0027 / ADR-0036), with the reason
/// each is excluded from [`ROUTE_MANIFEST`].
const CARVE_OUTS: &[(&str, &str)] = &[
    (
        "POST /api/studies/generate",
        "LLM orchestrator (ADR-0027): the client is the LLM",
    ),
    (
        "POST /api/studies/generate-danger-map",
        "LLM orchestrator (ADR-0027): the client is the LLM",
    ),
    (
        "* /api/assistant/*",
        "the assistant loop itself is the MCP client",
    ),
    (
        "* /api/auth/*",
        "session/infra, not a data or study operation",
    ),
    ("GET /api/whoami", "session/infra"),
    (
        "* /api/settings",
        "per-user UI prefs, not shared/study data",
    ),
    ("* /api/engines*", "engine admin/infra"),
    ("GET /api/health", "infra"),
    (
        "GET /api/engine/analyse",
        "WS streaming; `engine_analyse` covers the request/response form",
    ),
    ("* /.well-known/oauth-*", "OAuth discovery/infra"),
    ("* /oauth/*", "OAuth infra"),
];

/// Routes this issue's audit found un-mirrored but left out of scope
/// (pre-existing gaps, not introduced or widened by #183). Recorded so the
/// omission reads as a decision; closing these is tracked as future work, not
/// asserted here.
#[allow(dead_code)]
const KNOWN_GAPS: &[&str] = &[
    "GET/POST /api/databases, GET/PATCH/DELETE /api/databases/{id} — database CRUD",
    "PATCH /api/studies/{id} — rename",
    "DELETE /api/studies/{id} — delete",
    "DELETE /api/studies/{id}/nodes/{node_id} — delete_node",
    "POST /api/studies/add-line — add_line",
    "POST /api/studies/{id}/mark-transpositions — mark_transpositions",
    "POST /api/games/{id}/analyse — the #119 engine review of a *stored* game (distinct from analyse_game, which takes raw PGN)",
];

#[cfg(test)]
mod tests {
    use super::super::tools::default_registry;
    use super::*;

    #[test]
    fn every_manifest_entry_has_a_live_tool() {
        let registry = default_registry();
        let list = registry.list();
        let tools = list["tools"].as_array().expect("tools array");
        for entry in ROUTE_MANIFEST {
            assert!(
                tools.iter().any(|t| t["name"] == entry.tool),
                "{} {} claims tool `{}`, but no such tool is registered",
                entry.method,
                entry.path,
                entry.tool
            );
        }
    }

    #[test]
    fn carve_outs_and_known_gaps_are_documented() {
        // Not a runtime check against the router (Axum doesn't expose one) —
        // this just guards against the lists being silently emptied.
        assert!(!CARVE_OUTS.is_empty());
        assert!(!KNOWN_GAPS.is_empty());
    }
}

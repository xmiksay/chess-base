# 0006 — Frontend board: chessground + chess.js

**Context.** We need a chess board UI with drag-and-drop, legal-move highlighting
and arrows, plus client-side move legality for studies and play.

**Decision.** Use **chessground** (Lichess's board component) for rendering and
interaction, and **chess.js** for client-side legality/move generation — the same
battle-tested combination already used in the `f13/chess` project. The backend's
`shakmaty` remains the source of truth for validation and hashing.

**Consequences.** Mature, well-documented board UX with minimal custom code. Two
move libraries exist (chess.js on the client, shakmaty on the server); the server
is authoritative, the client copy is for responsiveness.

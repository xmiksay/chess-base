//! A named, ownable collection of games. `owner_id == NULL` means a global
//! (admin-managed) database searchable by every user.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "databases")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// `None` ⇒ global/admin-owned, searchable by all users.
    pub owner_id: Option<String>,
    pub name: String,
    /// `lichess` | `chesscom` | `master` | `own`.
    pub kind: String,
    /// Position-index depth policy (ADR-0003): `None` ⇒ index every ply;
    /// `Some(n)` ⇒ cap `position_index` population to the first `n` plies.
    pub index_depth: Option<i32>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

/// The four collection kinds a database may have (`databases.kind`).
pub const KINDS: [&str; 4] = ["lichess", "chesscom", "master", "own"];

/// Whether `kind` is one of the four known collection kinds (`KINDS`).
pub fn is_valid_kind(kind: &str) -> bool {
    KINDS.contains(&kind)
}

/// Default `index_depth` cap for a `master`/global database — the opening and
/// early middlegame, where opening-explorer value concentrates (ADR-0003).
pub const MASTER_INDEX_DEPTH: i32 = 36;

/// The `index_depth` a freshly created database of the given `kind` should get:
/// `Some(MASTER_INDEX_DEPTH)` for `master` (capped), `None` (full per-ply
/// indexing) for a user's own `lichess`/`chesscom`/`own` databases.
pub fn default_index_depth(kind: &str) -> Option<i32> {
    match kind {
        "master" => Some(MASTER_INDEX_DEPTH),
        _ => None,
    }
}

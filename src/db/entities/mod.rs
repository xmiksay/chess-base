//! SeaORM entities for the core domain, one module per table: the scaffold
//! `settings` key/value store and the ownable `databases` collections, plus
//! `games` + their `players`/`events` headers, the Zobrist `position_index`
//! (ADR-0003) and `studies` (a serialized `pgn_tree::MoveTree`). Relations are left
//! empty (`enum Relation {}`) — joins are issued explicitly by the query layer.

pub mod databases;
pub mod events;
pub mod games;
pub mod oauth_clients;
pub mod oauth_codes;
pub mod oauth_tokens;
pub mod players;
pub mod position_index;
pub mod service_tokens;
pub mod sessions;
pub mod settings;
pub mod studies;
pub mod users;

#[cfg(test)]
mod tests {
    use super::{databases, position_index};

    #[test]
    fn zobrist_cast_round_trips() {
        // High-bit-set value would overflow a naive i64 conversion; the bitwise
        // reinterpret must round-trip it.
        for z in [
            0u64,
            1,
            u64::MAX,
            0x8000_0000_0000_0000,
            0xDEAD_BEEF_CAFE_F00D,
        ] {
            assert_eq!(position_index::from_i64(position_index::to_i64(z)), z);
        }
    }

    #[test]
    fn index_depth_defaults_by_kind() {
        assert_eq!(
            databases::default_index_depth("master"),
            Some(databases::MASTER_INDEX_DEPTH)
        );
        assert_eq!(databases::default_index_depth("own"), None);
        assert_eq!(databases::default_index_depth("lichess"), None);
        assert_eq!(databases::default_index_depth("chesscom"), None);
    }
}

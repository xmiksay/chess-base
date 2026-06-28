//! SeaORM entities. The schema is intentionally minimal for the scaffold:
//! `settings` (key/value) and `databases` (the ownable game collections from
//! the multi-tenancy model). Games, positions, studies etc. are added by the
//! feature issues.

/// Key/value application + user settings.
pub mod settings {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "settings")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub key: String,
        pub value: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// A named, ownable collection of games. `owner_id == NULL` means a global
/// (admin-managed) database searchable by every user.
pub mod databases {
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
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

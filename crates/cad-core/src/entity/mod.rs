//! エンティティとその格納庫。

pub mod id;
pub mod kind;
pub mod store;

pub use id::EntityId;
pub use kind::{Entity, Geometry};
pub use store::EntityStore;

//! A narrow client for an Immich server.
//!
//! ⚠ **Zero GUI, and no knowledge of Rustroom's types.** It speaks Immich's JSON and returns
//! plain data; mapping that onto `CatalogEntry`/`PhotoId` is `app`'s job, in `immich_source`.

pub mod dto;
pub mod error;

pub use dto::{About, AlbumDetail, AlbumSummary, Asset, AssetType};
pub use error::Error;

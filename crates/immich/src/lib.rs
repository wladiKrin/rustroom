//! A narrow client for an Immich server.
//!
//! ⚠ **Zero GUI, and no knowledge of Rustroom's types.** It speaks Immich's JSON and returns
//! plain data; mapping that onto `CatalogEntry`/`PhotoId` is `app`'s job, in `immich_source`.

pub mod error;

pub use error::Error;

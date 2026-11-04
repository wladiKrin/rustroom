//! File → Print… (M10.18e): the selection on pages — one photo per page or a contact sheet — written as a
//! PDF or sent to a printer through CUPS. No egui.
//!
//! ⚠⚠ A page shows the export's render, never a second one: each photo goes through `batch_export`'s worker
//! into a staging folder at its printed size × the PPI, and [`pdf`] embeds those JPEGs as they are.

pub mod job;
pub mod layout;
pub mod pdf;
pub mod printer;

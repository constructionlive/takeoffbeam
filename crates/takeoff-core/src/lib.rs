//! The takeoff data model shared by the viewer, the agent and the sidecar file.
//!
//! COORDINATE RULE: every position is a fraction (0–1) of the FULL page. Never
//! pixels, never a model's 0–1000 grid, never local to a tile or a zoomed view.
//! Pixels exist only at render time (page size at scale × zoom).
//!
//! The JSON shape (`TakeoffData`) is construction.live's `takeoffTypes.ts`
//! plus optional extensions (annotations, viewports, item metadata, display
//! settings), so a platform file loads here unchanged and a Takeoff Desk file
//! imports into the platform (which keeps the parts it knows).

pub mod coords;
pub mod docmap;
pub mod doc;
pub mod geometry;
pub mod legend;
pub mod list;
pub mod model;
pub mod normalize;
pub mod numbering;
pub mod pages;
pub mod set;
pub mod sheets;
pub mod sidecar;
pub mod slip;
pub mod snap;
pub mod spaces;
pub mod time;
pub mod units;

pub use doc::{Clipboard, Document, ItemRef};
pub use model::*;

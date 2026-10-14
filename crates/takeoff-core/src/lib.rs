//! The takeoff data model shared by the viewer, the agent and the sidecar file.
//!
//! COORDINATE RULE: every position is a fraction (0–1) of the FULL page. Never
//! pixels, never a model's 0–1000 grid, never local to a tile or a zoomed view.
//! Pixels exist only at render time (page size at scale × zoom).
//!
//! The JSON shape (`TakeoffData`) is the same as construction.live's
//! `takeoffTypes.ts`, so a sidecar file imports into the platform unchanged.

pub mod geometry;
pub mod model;
pub mod numbering;

pub use model::*;

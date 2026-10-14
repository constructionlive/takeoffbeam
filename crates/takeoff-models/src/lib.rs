//! Local models for Takeoff Desk: read GGUF headers, check whether a model
//! fits this machine before loading it, and (later) run llama-server.

pub mod fit;
pub mod gguf;
pub mod sys;

//! Core DAG configuration, step, resource, and result types.
//!
//! The model now lives in the model crate; this module re-exports it so every
//! existing model path keeps working.

pub use dagrun_model::model::*;

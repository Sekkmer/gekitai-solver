//! Gekitai rules, search engines, and independently checkable strategy certificates.
//!
//! The browser player in `web/` is a separate JavaScript implementation. Its
//! rules and canonical keys are checked against this library in the test suite.
//! See `docs/RULES.md` for the exact terminal and repetition conventions.

pub mod board;
pub mod certificate;
pub mod certificate_work;
pub mod checkpoint;
pub mod forcing;
pub mod movegen;
pub mod player;
pub mod player_server;
pub mod position;
pub mod proof;
pub mod rules;
pub mod search;
pub mod stats;
pub mod symmetry;
pub mod trainer;
pub mod tt;

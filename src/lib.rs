//! vivido-mcp library surface.
//!
//! The binary is a thin CLI over this library so integration tests can drive
//! the same [`Server`](server::Server) the MCP loop uses.

pub mod endpoint;
pub mod events;
pub mod headless;
pub mod image_tools;
pub mod ipc;
pub mod mcp;
pub mod prompts;
pub mod sanitize;
pub mod server;
pub mod tiers;
pub mod tools;

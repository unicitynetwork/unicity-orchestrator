//! Tool handler registry for managing MCP tool implementations.
//!
//! This module provides a simple way to register and invoke tool handlers,
//! making it easy to add new tools without modifying the core `ServerHandler`
//! implementation.

mod registry;

pub use registry::{ToolContext, ToolHandler, ToolRegistry};

// Tool handler implementations
mod execute_tool;
mod list_discovered_tools;
mod plan_tools;
mod select_tool;

pub use execute_tool::ExecuteToolHandler;
pub use list_discovered_tools::ListDiscoveredToolsHandler;
pub use plan_tools::PlanToolsHandler;
pub use select_tool::SelectToolHandler;

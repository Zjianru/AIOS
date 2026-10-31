//! 内置只读工具（骨架）：read、glob、grep 的实现提交里补。

use serde_json::{Value, json};

use crate::{Tool, ToolCall, ToolConcurrency, ToolContext, ToolFailure, ToolFuture, ToolResult};

macro_rules! stub_tool {
    ($name:ident, $wire:literal, $description:literal) => {
        /// 内置工具（骨架）。
        pub struct $name {
            parameters: Value,
        }

        impl $name {
            pub fn new() -> Self {
                Self {
                    parameters: json!({"type": "object", "properties": {}}),
                }
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl Tool for $name {
            fn name(&self) -> &str {
                $wire
            }

            fn description(&self) -> &str {
                $description
            }

            fn parameters(&self) -> &Value {
                &self.parameters
            }

            fn concurrency(&self) -> ToolConcurrency {
                ToolConcurrency::Parallel
            }

            fn execute<'a>(
                &'a self,
                call: ToolCall,
                _context: &'a ToolContext,
            ) -> ToolFuture<'a, ToolResult> {
                Box::pin(async move {
                    ToolResult::error(call.id, ToolFailure::plain("tool not implemented yet"))
                })
            }
        }
    };
}

stub_tool!(
    ReadTool,
    "read",
    "Read a UTF-8 text file and return line-numbered content."
);
stub_tool!(
    GlobTool,
    "glob",
    "Find files whose paths match a glob pattern."
);
stub_tool!(
    GrepTool,
    "grep",
    "Search file contents with a regular expression."
);

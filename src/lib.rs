pub mod agent;
pub mod config;
pub mod events;
pub mod file;
pub mod helpers;
pub mod log;
pub mod notifications;
pub mod process;

// Deprecated
// #[cxx::bridge]
// pub mod service {
//     #[repr(u8)]
//     enum Fork {
//         Parent,
//         Child,
//     }

//     pub struct ProcessMetadata {
//         pub name: String,
//         pub shell: String,
//         pub command: String,
//         pub log_path: String,
//         pub args: Vec<String>,
//         pub env: Vec<String>,
//     }
// }

// Re-export Rust implementations outside of cxx bridge
pub use process::{
    get_process_cpu_usage_percentage, get_process_cpu_usage_percentage_fast,
    get_process_cpu_usage_with_children, get_process_cpu_usage_with_children_fast,
    get_process_cpu_usage_with_children_from_process, get_process_memory_with_children,
    process_find_children, process_run, process_stop,
};

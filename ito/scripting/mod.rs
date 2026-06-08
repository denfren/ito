//! Rhai scripting binding.
//!
//! The YAML-specific cursor / fs / lint / resolver layer has moved
//! under `crate::yamlito::scripting`. Only the generic runner support
//! (typed script args, virtual filesystem) lives here.

pub mod args;
pub mod hcl;
pub mod j2;
pub mod json;
pub mod path;
pub mod pathmap;
pub mod proc;
pub mod re;
pub mod string;
pub mod toml;
pub mod version;
pub mod vfs;
pub mod yaml;

pub use args::ScriptArg;
pub use pathmap::PathMapper;
pub use vfs::Vfs;

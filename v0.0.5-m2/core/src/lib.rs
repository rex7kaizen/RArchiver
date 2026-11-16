//! RArchiver çekirdeği: Android'den bağımsız, saf Rust.

pub mod archive;
pub mod browser;
pub mod error;
pub mod filetype;
pub mod fsops;
pub mod job;
pub mod listing;
pub mod search;
pub mod settings;

pub use error::{Error, Result};
pub use filetype::{badge_label, classify, FileClass};
pub use job::{CancelToken, Ctx, Progress, ProgressSnapshot};
pub use listing::*;
pub use settings::{Settings, ViewMode};

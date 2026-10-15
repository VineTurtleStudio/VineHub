pub mod core;
pub mod rpc;
pub mod ui;
pub mod verifiers;

pub use core::dispatch::{MagicKind, detect_magic, expected_by_ext, verify_file};
pub use core::entropy;
pub use core::hash::{self, HashKind, ManifestEntry};
pub use core::walker;
pub use core::{FileHint, Finding, Mode, Opts, Report, Severity, Verifier};

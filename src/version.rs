//! Build version shared by stable and preview distributions.

/// The version embedded in this build.
///
/// Stable builds use Cargo's package version. Preview builds set
/// `CRABCODE_VERSION` during compilation (for example, `0.0.30812588153`).
pub const CURRENT: &str = env!("CRABCODE_VERSION");

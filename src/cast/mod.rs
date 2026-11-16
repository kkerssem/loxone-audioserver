//! Google Cast playback for radio zones.

#[cfg(feature = "miniserver")]
mod announce;
mod device;
mod discovery;
mod id;
#[cfg(feature = "miniserver")]
mod manager;
mod matcher;
#[cfg(feature = "miniserver")]
mod session;
#[cfg(feature = "miniserver")]
mod sync;

#[cfg(feature = "miniserver")]
pub use announce::{play_announcement, Announcement};
#[cfg(feature = "miniserver")]
pub use manager::CastPlaybackManager;
#[cfg(feature = "miniserver")]
pub use matcher::warm_known_cast_devices;
#[cfg(feature = "miniserver")]
pub use sync::CastRemoteUpdate;

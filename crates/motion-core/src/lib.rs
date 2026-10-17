//! Product-owned project model, exact time and persistence.

pub mod persistence;
pub mod project;
pub mod time;

pub use project::{Canvas, Clip, Interpolation, Keyframe, MediaAsset, MediaKind, Project, ProjectError, StableId, StreamMetadata, Track, TrackKind, Transform, TransformKeyframes, Vec2};
pub use time::{FrameRate, Tick, TimeBase, TICKS_PER_SECOND};

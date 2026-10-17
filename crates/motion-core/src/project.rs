use crate::time::{FrameRate, Tick, TimeBase};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;
use uuid::Uuid;

pub type StableId = Uuid;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: StableId,
    pub name: String,
    pub canvas: Canvas,
    pub frame_rate: FrameRate,
    #[serde(default)] pub assets: Vec<MediaAsset>,
    #[serde(default)] pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Canvas { pub width: u32, pub height: u32 }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind { Image, Video, Audio }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaAsset {
    pub id: StableId,
    /// Product-owned locator (typically a path or URI); no media is embedded.
    pub locator: String,
    pub kind: MediaKind,
    pub duration: Option<Tick>,
    #[serde(default)] pub streams: Vec<StreamMetadata>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StreamMetadata {
    pub stream_index: u32,
    pub kind: MediaKind,
    /// Original decoder clock: PTS * time_base numerator / denominator seconds.
    pub time_base: TimeBase,
    pub first_pts: Option<i64>,
    pub duration_pts: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind { Visual, Audio }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: StableId,
    pub name: String,
    pub kind: TrackKind,
    /// Lower visual tracks are composited first; vector order is bottom-to-top.
    #[serde(default)] pub clips: Vec<Clip>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: StableId,
    pub asset_id: StableId,
    pub timeline_start: Tick,
    pub source_in: Tick,
    pub duration: Tick,
    pub transform: Transform,
    #[serde(default)] pub keyframes: TransformKeyframes,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec2 { pub x: f64, pub y: f64 }

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub position: Vec2,
    pub anchor: Vec2,
    pub scale: Vec2,
    pub rotation_degrees: f64,
    pub opacity: f64,
}

impl Default for Transform {
    fn default() -> Self { Self { position: Vec2::default(), anchor: Vec2 { x: 0.5, y: 0.5 }, scale: Vec2 { x: 1.0, y: 1.0 }, rotation_degrees: 0.0, opacity: 1.0 } }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation { Linear, EaseIn, EaseOut, EaseInOut }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe<T> { pub time: Tick, pub value: T, pub interpolation: Interpolation }

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TransformKeyframes {
    #[serde(default)] pub position: Vec<Keyframe<Vec2>>,
    #[serde(default)] pub anchor: Vec<Keyframe<Vec2>>,
    #[serde(default)] pub scale: Vec<Keyframe<Vec2>>,
    #[serde(default)] pub rotation_degrees: Vec<Keyframe<f64>>,
    #[serde(default)] pub opacity: Vec<Keyframe<f64>>,
}

#[derive(Debug, Error, PartialEq)]
pub enum ProjectError {
    #[error("canvas dimensions must be nonzero")]
    InvalidCanvas,
    #[error("frame rate must have a nonzero numerator and denominator")]
    InvalidFrameRate,
    #[error("identifier is duplicated: {0}")]
    DuplicateId(StableId),
    #[error("asset locator must not be empty")]
    EmptyLocator,
    #[error("asset duration must be positive")]
    InvalidAssetDuration,
    #[error("stream time base must be valid")]
    InvalidTimeBase,
    #[error("clip duration must be positive and start/source_in nonnegative")]
    InvalidClipTiming,
    #[error("clip references missing asset {0}")]
    MissingAsset(StableId),
    #[error("clip kind is incompatible with track kind")]
    IncompatibleTrack,
    #[error("transform contains non-finite values or opacity outside [0,1]")]
    InvalidTransform,
    #[error("keyframe times must be nonnegative and strictly increasing")]
    InvalidKeyframes,
}

impl Project {
    /// Validate cross-references and invariants before save or use.
    pub fn validate(&self) -> Result<(), ProjectError> {
        if self.canvas.width == 0 || self.canvas.height == 0 { return Err(ProjectError::InvalidCanvas); }
        if !self.frame_rate.validate() { return Err(ProjectError::InvalidFrameRate); }
        let mut ids = HashSet::new();
        insert_id(&mut ids, self.id)?;
        for a in &self.assets {
            insert_id(&mut ids, a.id)?;
            if a.locator.trim().is_empty() { return Err(ProjectError::EmptyLocator); }
            if a.duration.is_some_and(|d| d.0 <= 0) { return Err(ProjectError::InvalidAssetDuration); }
            for s in &a.streams { if !s.time_base.validate() { return Err(ProjectError::InvalidTimeBase); } }
        }
        for t in &self.tracks {
            insert_id(&mut ids, t.id)?;
            for c in &t.clips {
                insert_id(&mut ids, c.id)?;
                if c.timeline_start.0 < 0 || c.source_in.0 < 0 || c.duration.0 <= 0
                    || c.timeline_start.checked_add(c.duration).is_none()
                    || c.source_in.checked_add(c.duration).is_none()
                { return Err(ProjectError::InvalidClipTiming); }
                let asset = self.assets.iter().find(|a| a.id == c.asset_id).ok_or(ProjectError::MissingAsset(c.asset_id))?;
                let kind = asset.kind;
                if asset.duration.is_some_and(|d| c.source_in.0.checked_add(c.duration.0).is_none_or(|end| end > d.0)) { return Err(ProjectError::InvalidClipTiming); }
                let compatible = match t.kind { TrackKind::Visual => kind != MediaKind::Audio, TrackKind::Audio => kind == MediaKind::Audio };
                if !compatible { return Err(ProjectError::IncompatibleTrack); }
                validate_transform(c.transform)?;
                validate_keys(&c.keyframes.position)?; validate_keys(&c.keyframes.anchor)?;
                validate_keys(&c.keyframes.scale)?; validate_keys(&c.keyframes.rotation_degrees)?;
                validate_keys(&c.keyframes.opacity)?;
                if c.keyframes.position.iter().any(|k| !finite2(k.value)) || c.keyframes.anchor.iter().any(|k| !finite2(k.value)) || c.keyframes.scale.iter().any(|k| !finite2(k.value)) || c.keyframes.rotation_degrees.iter().any(|k| !k.value.is_finite()) || c.keyframes.opacity.iter().any(|k| !k.value.is_finite() || !(0.0..=1.0).contains(&k.value)) { return Err(ProjectError::InvalidTransform); }
            }
        }
        Ok(())
    }
}

fn insert_id(ids: &mut HashSet<StableId>, id: StableId) -> Result<(), ProjectError> { if ids.insert(id) { Ok(()) } else { Err(ProjectError::DuplicateId(id)) } }
fn finite2(v: Vec2) -> bool { v.x.is_finite() && v.y.is_finite() }
fn validate_transform(t: Transform) -> Result<(), ProjectError> {
    if !finite2(t.position) || !finite2(t.anchor) || !finite2(t.scale) || !t.rotation_degrees.is_finite() || !t.opacity.is_finite() || !(0.0..=1.0).contains(&t.opacity) { Err(ProjectError::InvalidTransform) } else { Ok(()) }
}
fn validate_keys<T>(keys: &[Keyframe<T>]) -> Result<(), ProjectError> {
    if keys.iter().any(|k| k.time.0 < 0) || keys.windows(2).any(|w| w[0].time >= w[1].time) { Err(ProjectError::InvalidKeyframes) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn valid_project() -> Project {
        let asset = Uuid::new_v4();
        Project { id: Uuid::new_v4(), name: "test".into(), canvas: Canvas { width: 1920, height: 1080 }, frame_rate: FrameRate::new(30, 1).unwrap(), assets: vec![MediaAsset { id: asset, locator: "clip.mp4".into(), kind: MediaKind::Video, duration: Some(Tick(10_000)), streams: vec![] }], tracks: vec![Track { id: Uuid::new_v4(), name: "Video".into(), kind: TrackKind::Visual, clips: vec![Clip { id: Uuid::new_v4(), asset_id: asset, timeline_start: Tick::ZERO, source_in: Tick::ZERO, duration: Tick(1_000), transform: Transform::default(), keyframes: TransformKeyframes::default() }] }] }
    }
    #[test] fn valid_project_passes_and_missing_asset_fails() {
        let mut p = valid_project(); assert_eq!(p.validate(), Ok(()));
        p.tracks[0].clips[0].asset_id = Uuid::new_v4(); assert!(matches!(p.validate(), Err(ProjectError::MissingAsset(_))));
    }
    #[test] fn rejects_unsorted_keyframes_and_bad_opacity() {
        let mut p = valid_project();
        p.tracks[0].clips[0].keyframes.opacity = vec![Keyframe { time: Tick(5), value: 0.5, interpolation: Interpolation::Linear }, Keyframe { time: Tick(4), value: 0.4, interpolation: Interpolation::Linear }];
        assert_eq!(p.validate(), Err(ProjectError::InvalidKeyframes));
        p.tracks[0].clips[0].keyframes.opacity.clear(); p.tracks[0].clips[0].transform.opacity = 2.0;
        assert_eq!(p.validate(), Err(ProjectError::InvalidTransform));
    }
    #[test] fn rejects_timeline_end_overflow() {
        let mut p = valid_project();
        p.tracks[0].clips[0].timeline_start = Tick(i64::MAX - 5);
        p.tracks[0].clips[0].duration = Tick(10);
        assert_eq!(p.validate(), Err(ProjectError::InvalidClipTiming));
    }
}

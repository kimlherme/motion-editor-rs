//! Product session and commands that connect the editor UI to the shared model.
use motion_core::{
    persistence,
    project::{Canvas, Clip, Interpolation, Keyframe, MediaAsset, MediaKind, Project, Track, TrackKind, Transform, Vec2},
    time::{FrameRate, Tick},
};
use motion_media::{FrameProvider, MediaError, RgbaFrame};
use motion_render::render_frame;
use std::{path::Path, sync::Arc};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("project: {0}")]
    Project(#[from] motion_core::ProjectError),
    #[error("project persistence: {0}")]
    Persistence(#[from] persistence::PersistenceError),
    #[error("media: {0}")]
    Media(#[from] MediaError),
    #[error("preview failed: {0}")]
    Preview(String),
    #[error("invalid duration")]
    InvalidDuration,
    #[error("asset was not found")]
    MissingAsset,
    #[error("clip was not found")]
    MissingClip,
    #[error("project has no track for this media type")]
    MissingTrack,
    #[error("the selected clip is not active at the playhead")]
    ClipNotActive,
    #[error("keyframe value is outside the supported range")]
    InvalidKeyframeValue,
}

/// Mutable UI-facing session. Rendering consumes an immutable project snapshot.
pub struct EditorSession {
    project: Project,
    selected_clip: Option<Uuid>,
    playhead: Tick,
    revision: u64,
}

impl EditorSession {
    pub fn new(project: Project) -> Self {
        Self { project, selected_clip: None, playhead: Tick::ZERO, revision: 0 }
    }

    pub fn project(&self) -> &Project { &self.project }
    pub fn snapshot(&self) -> Arc<Project> { Arc::new(self.project.clone()) }
    pub fn selected_clip(&self) -> Option<Uuid> { self.selected_clip }
    pub fn playhead(&self) -> Tick { self.playhead }
    pub fn revision(&self) -> u64 { self.revision }

    pub fn new_project(name: impl Into<String>, width: u32, height: u32, frame_rate: FrameRate) -> Result<Self, EngineError> {
        let project = Project {
            id: Uuid::new_v4(), name: name.into(), canvas: Canvas { width, height }, frame_rate,
            assets: Vec::new(), tracks: vec![Track { id: Uuid::new_v4(), name: "Vídeo 1".into(), kind: TrackKind::Visual, clips: Vec::new() }],
        };
        project.validate()?;
        Ok(Self::new(project))
    }

    pub fn select_clip(&mut self, id: Uuid) -> Result<(), EngineError> {
        if !self.project.tracks.iter().flat_map(|t| t.clips.iter()).any(|c| c.id == id) { return Err(EngineError::MissingClip); }
        self.selected_clip = Some(id);
        Ok(())
    }

    pub fn set_playhead(&mut self, time: Tick) { self.playhead = Tick(time.0.max(0)); }

    pub fn selected_source_time(&self) -> Result<Tick, EngineError> {
        let id = self.selected_clip.ok_or(EngineError::MissingClip)?;
        let clip = self.project.tracks.iter().flat_map(|t| &t.clips).find(|c| c.id == id).ok_or(EngineError::MissingClip)?;
        let end = clip.timeline_start.checked_add(clip.duration).ok_or(EngineError::ClipNotActive)?;
        if self.playhead < clip.timeline_start || self.playhead >= end { return Err(EngineError::ClipNotActive); }
        clip.source_in.checked_add(Tick(self.playhead.0 - clip.timeline_start.0)).ok_or(EngineError::ClipNotActive)
    }

    pub fn selected_transform(&self) -> Result<Transform, EngineError> {
        let id = self.selected_clip.ok_or(EngineError::MissingClip)?;
        let source_time = self.selected_source_time()?;
        let clip = self.project.tracks.iter().flat_map(|t| &t.clips).find(|c| c.id == id).ok_or(EngineError::MissingClip)?;
        Ok(motion_animation::evaluate_transform(clip, source_time))
    }

    pub fn set_selected_position_keyframe(&mut self, value: Vec2, interpolation: Interpolation) -> Result<(), EngineError> {
        if !value.x.is_finite() || !value.y.is_finite() { return Err(EngineError::InvalidKeyframeValue); }
        self.write_selected_keyframe(|keys| &mut keys.position, value, interpolation)
    }

    pub fn set_selected_scale_keyframe(&mut self, value: Vec2, interpolation: Interpolation) -> Result<(), EngineError> {
        if !value.x.is_finite() || !value.y.is_finite() { return Err(EngineError::InvalidKeyframeValue); }
        self.write_selected_keyframe(|keys| &mut keys.scale, value, interpolation)
    }

    pub fn set_selected_rotation_keyframe(&mut self, value: f64, interpolation: Interpolation) -> Result<(), EngineError> {
        if !value.is_finite() { return Err(EngineError::InvalidKeyframeValue); }
        self.write_selected_keyframe(|keys| &mut keys.rotation_degrees, value, interpolation)
    }

    pub fn set_selected_opacity_keyframe(&mut self, value: f64, interpolation: Interpolation) -> Result<(), EngineError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) { return Err(EngineError::InvalidKeyframeValue); }
        self.write_selected_keyframe(|keys| &mut keys.opacity, value, interpolation)
    }

    fn write_selected_keyframe<T: Copy>(
        &mut self,
        list: for<'a> fn(&'a mut motion_core::project::TransformKeyframes) -> &'a mut Vec<Keyframe<T>>,
        value: T,
        interpolation: Interpolation,
    ) -> Result<(), EngineError> {
        let id = self.selected_clip.ok_or(EngineError::MissingClip)?;
        let time = self.selected_source_time()?;
        let clip = self.project.tracks.iter_mut().flat_map(|t| &mut t.clips).find(|c| c.id == id).ok_or(EngineError::MissingClip)?;
        let frames = list(&mut clip.keyframes);
        let key = Keyframe { time, value, interpolation };
        match frames.binary_search_by_key(&time, |frame| frame.time) {
            Ok(index) => frames[index] = key,
            Err(index) => frames.insert(index, key),
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }

    /// Imports PNG/JPEG metadata and inserts a five-second still clip at the playhead.
    pub fn import_image(&mut self, path: impl AsRef<Path>) -> Result<Uuid, EngineError> {
        let path = path.as_ref();
        motion_media::probe_still(path)?;
        let visual_track = self.project.tracks.iter().position(|t| t.kind == TrackKind::Visual).ok_or(EngineError::MissingTrack)?;
        let asset = MediaAsset { id: Uuid::new_v4(), locator: path.to_string_lossy().into_owned(), kind: MediaKind::Image, duration: None, streams: Vec::new() };
        let duration = Tick::from_rate_units(5, self.project.frame_rate).ok_or(EngineError::InvalidDuration)?;
        let clip = Clip { id: Uuid::new_v4(), asset_id: asset.id, timeline_start: self.playhead, source_in: Tick::ZERO, duration, transform: Transform::default(), keyframes: Default::default() };
        self.project.assets.push(asset.clone());
        self.project.tracks[visual_track].clips.push(clip);
        self.revision = self.revision.wrapping_add(1);
        Ok(asset.id)
    }

    pub fn add_clip(&mut self, asset_id: Uuid, timeline_start: Tick, source_in: Tick, duration: Tick) -> Result<Uuid, EngineError> {
        if duration.0 <= 0 || timeline_start.0 < 0 || source_in.0 < 0 { return Err(EngineError::Project(motion_core::ProjectError::InvalidClipTiming)); }
        let asset = self.project.assets.iter().find(|a| a.id == asset_id).ok_or(EngineError::MissingAsset)?;
        let track_kind = if asset.kind == MediaKind::Audio { TrackKind::Audio } else { TrackKind::Visual };
        let track = self.project.tracks.iter_mut().find(|t| t.kind == track_kind).ok_or(EngineError::MissingTrack)?;
        let clip = Clip { id: Uuid::new_v4(), asset_id, timeline_start, source_in, duration, transform: Transform::default(), keyframes: Default::default() };
        let id = clip.id;
        track.clips.push(clip);
        self.revision = self.revision.wrapping_add(1);
        Ok(id)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), EngineError> { persistence::save_atomic(path, &self.project)?; Ok(()) }
    pub fn open(path: impl AsRef<Path>) -> Result<Self, EngineError> { Ok(Self::new(persistence::load(path)?)) }

    pub fn preview<P: FrameProvider>(&self, provider: &P) -> Result<RgbaFrame, EngineError> {
        render_frame(&self.project, self.playhead, provider).map_err(|e| EngineError::Preview(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_session_starts_at_zero_and_has_valid_snapshot() {
        let session = EditorSession::new_project("Untitled", 1920, 1080, FrameRate::new(30, 1).unwrap()).unwrap();
        assert_eq!(session.playhead(), Tick::ZERO);
        assert_eq!(session.snapshot().canvas.width, session.project().canvas.width);
        assert_eq!(session.project().tracks.len(), 1);
    }

    #[test]
    fn adding_a_clip_assigns_an_id_and_selects_it() {
        let mut session = EditorSession::new_project("Untitled", 640, 360, FrameRate::new(30, 1).unwrap()).unwrap();
        let asset_id = Uuid::new_v4();
        session.project.assets.push(MediaAsset {
            id: asset_id, locator: "still.png".into(), kind: MediaKind::Image,
            duration: None, streams: Vec::new(),
        });
        let clip_id = session.add_clip(asset_id, Tick(10), Tick(7), Tick(100)).unwrap();
        session.select_clip(clip_id).unwrap();
        session.set_playhead(Tick(15));
        assert_eq!(session.selected_source_time().unwrap(), Tick(12));
        session.set_selected_position_keyframe(Vec2 { x: 8.0, y: -3.0 }, Interpolation::EaseInOut).unwrap();
        assert_eq!(session.selected_clip(), Some(clip_id));
        assert_eq!(session.project.tracks[0].clips[0].asset_id, asset_id);
        assert_eq!(session.project.tracks[0].clips[0].keyframes.position[0].time, Tick(12));
        assert_eq!(session.project.tracks[0].clips[0].keyframes.position[0].interpolation, Interpolation::EaseInOut);
        session.set_selected_position_keyframe(Vec2 { x: 9.0, y: 4.0 }, Interpolation::Linear).unwrap();
        let position_keys = &session.project.tracks[0].clips[0].keyframes.position;
        assert_eq!(position_keys.len(), 1);
        assert_eq!(position_keys[0].value, Vec2 { x: 9.0, y: 4.0 });
        assert_eq!(position_keys[0].interpolation, Interpolation::Linear);
        assert_eq!(session.revision(), 3);
        session.set_playhead(Tick(110));
        assert!(matches!(session.set_selected_opacity_keyframe(0.5, Interpolation::Linear), Err(EngineError::ClipNotActive)));
    }
}

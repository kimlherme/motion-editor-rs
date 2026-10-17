//! Versioned JSON project file and same-directory atomic replacement.
use crate::project::{Project, ProjectError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;

const FORMAT_VERSION: u32 = 1;

/// External, versioned DTO envelope, kept distinct from the runtime model.
#[derive(Serialize, Deserialize)]
struct ProjectFileDto { format: String, version: u32, project: Value }

/// Persisted schema types are deliberately independent from runtime types.
#[derive(Serialize, Deserialize)]
struct ProjectDto {
    id: uuid::Uuid,
    name: String,
    canvas: CanvasDto,
    frame_rate: FrameRateDto,
    #[serde(default)] assets: Vec<MediaAssetDto>,
    #[serde(default)] tracks: Vec<TrackDto>,
}

#[derive(Serialize, Deserialize)]
struct TickDto(i64);
#[derive(Serialize, Deserialize)]
struct FrameRateDto { numerator: u32, denominator: u32 }
#[derive(Serialize, Deserialize)]
struct TimeBaseDto { numerator: u32, denominator: u32 }
#[derive(Serialize, Deserialize)]
struct CanvasDto { width: u32, height: u32 }
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MediaKindDto { Image, Video, Audio }
#[derive(Serialize, Deserialize)]
struct MediaAssetDto {
    id: uuid::Uuid,
    locator: String,
    kind: MediaKindDto,
    duration: Option<TickDto>,
    #[serde(default)] streams: Vec<StreamMetadataDto>,
}
#[derive(Serialize, Deserialize)]
struct StreamMetadataDto {
    stream_index: u32,
    kind: MediaKindDto,
    time_base: TimeBaseDto,
    first_pts: Option<i64>,
    duration_pts: Option<i64>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TrackKindDto { Visual, Audio }
#[derive(Serialize, Deserialize)]
struct TrackDto {
    id: uuid::Uuid,
    name: String,
    kind: TrackKindDto,
    #[serde(default)] clips: Vec<ClipDto>,
}
#[derive(Serialize, Deserialize)]
struct ClipDto {
    id: uuid::Uuid,
    asset_id: uuid::Uuid,
    timeline_start: TickDto,
    source_in: TickDto,
    duration: TickDto,
    transform: TransformDto,
    #[serde(default)] keyframes: TransformKeyframesDto,
}
#[derive(Serialize, Deserialize)]
struct Vec2Dto { x: f64, y: f64 }
#[derive(Serialize, Deserialize)]
struct TransformDto {
    position: Vec2Dto,
    anchor: Vec2Dto,
    scale: Vec2Dto,
    rotation_degrees: f64,
    opacity: f64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum InterpolationDto { Linear, EaseIn, EaseOut, EaseInOut }
#[derive(Serialize, Deserialize)]
struct KeyframeDto<T> { time: TickDto, value: T, interpolation: InterpolationDto }
#[derive(Serialize, Deserialize)]
struct TransformKeyframesDto {
    #[serde(default)] position: Vec<KeyframeDto<Vec2Dto>>,
    #[serde(default)] anchor: Vec<KeyframeDto<Vec2Dto>>,
    #[serde(default)] scale: Vec<KeyframeDto<Vec2Dto>>,
    #[serde(default)] rotation_degrees: Vec<KeyframeDto<f64>>,
    #[serde(default)] opacity: Vec<KeyframeDto<f64>>,
}

use crate::project::{Canvas, Clip, Interpolation, Keyframe, MediaAsset, MediaKind, StreamMetadata, Track, TrackKind, Transform, TransformKeyframes, Vec2};
use crate::time::{FrameRate, Tick, TimeBase};

impl From<Tick> for TickDto { fn from(v: Tick) -> Self { Self(v.0) } }
impl From<TickDto> for Tick { fn from(v: TickDto) -> Self { Self(v.0) } }
impl From<FrameRate> for FrameRateDto { fn from(v: FrameRate) -> Self { Self { numerator: v.numerator, denominator: v.denominator } } }
impl From<FrameRateDto> for FrameRate { fn from(v: FrameRateDto) -> Self { Self { numerator: v.numerator, denominator: v.denominator } } }
impl From<TimeBase> for TimeBaseDto { fn from(v: TimeBase) -> Self { Self { numerator: v.numerator, denominator: v.denominator } } }
impl From<TimeBaseDto> for TimeBase { fn from(v: TimeBaseDto) -> Self { Self { numerator: v.numerator, denominator: v.denominator } } }
impl From<Canvas> for CanvasDto { fn from(v: Canvas) -> Self { Self { width: v.width, height: v.height } } }
impl From<CanvasDto> for Canvas { fn from(v: CanvasDto) -> Self { Self { width: v.width, height: v.height } } }

macro_rules! kind_conversion {
    ($runtime:path, $dto:path, {$($variant:ident),+ $(,)?}) => {
        impl From<$runtime> for $dto { fn from(v: $runtime) -> Self { match v { $(<$runtime>::$variant => <$dto>::$variant),+ } } }
        impl From<$dto> for $runtime { fn from(v: $dto) -> Self { match v { $(<$dto>::$variant => <$runtime>::$variant),+ } } }
    };
}
kind_conversion!(MediaKind, MediaKindDto, { Image, Video, Audio });
kind_conversion!(TrackKind, TrackKindDto, { Visual, Audio });
kind_conversion!(Interpolation, InterpolationDto, { Linear, EaseIn, EaseOut, EaseInOut });

impl From<MediaAsset> for MediaAssetDto {
    fn from(v: MediaAsset) -> Self { Self { id: v.id, locator: v.locator, kind: v.kind.into(), duration: v.duration.map(Into::into), streams: v.streams.into_iter().map(Into::into).collect() } }
}
impl From<MediaAssetDto> for MediaAsset {
    fn from(v: MediaAssetDto) -> Self { Self { id: v.id, locator: v.locator, kind: v.kind.into(), duration: v.duration.map(Into::into), streams: v.streams.into_iter().map(Into::into).collect() } }
}
impl From<StreamMetadata> for StreamMetadataDto {
    fn from(v: StreamMetadata) -> Self { Self { stream_index: v.stream_index, kind: v.kind.into(), time_base: v.time_base.into(), first_pts: v.first_pts, duration_pts: v.duration_pts } }
}
impl From<StreamMetadataDto> for StreamMetadata {
    fn from(v: StreamMetadataDto) -> Self { Self { stream_index: v.stream_index, kind: v.kind.into(), time_base: v.time_base.into(), first_pts: v.first_pts, duration_pts: v.duration_pts } }
}
impl From<Track> for TrackDto {
    fn from(v: Track) -> Self { Self { id: v.id, name: v.name, kind: v.kind.into(), clips: v.clips.into_iter().map(Into::into).collect() } }
}
impl From<TrackDto> for Track {
    fn from(v: TrackDto) -> Self { Self { id: v.id, name: v.name, kind: v.kind.into(), clips: v.clips.into_iter().map(Into::into).collect() } }
}
impl From<Clip> for ClipDto {
    fn from(v: Clip) -> Self { Self { id: v.id, asset_id: v.asset_id, timeline_start: v.timeline_start.into(), source_in: v.source_in.into(), duration: v.duration.into(), transform: v.transform.into(), keyframes: v.keyframes.into() } }
}
impl From<ClipDto> for Clip {
    fn from(v: ClipDto) -> Self { Self { id: v.id, asset_id: v.asset_id, timeline_start: v.timeline_start.into(), source_in: v.source_in.into(), duration: v.duration.into(), transform: v.transform.into(), keyframes: v.keyframes.into() } }
}
impl From<Vec2> for Vec2Dto { fn from(v: Vec2) -> Self { Self { x: v.x, y: v.y } } }
impl From<Vec2Dto> for Vec2 { fn from(v: Vec2Dto) -> Self { Self { x: v.x, y: v.y } } }
impl From<Transform> for TransformDto {
    fn from(v: Transform) -> Self { Self { position: v.position.into(), anchor: v.anchor.into(), scale: v.scale.into(), rotation_degrees: v.rotation_degrees, opacity: v.opacity } }
}
impl From<TransformDto> for Transform {
    fn from(v: TransformDto) -> Self { Self { position: v.position.into(), anchor: v.anchor.into(), scale: v.scale.into(), rotation_degrees: v.rotation_degrees, opacity: v.opacity } }
}
impl<T, U> From<Keyframe<T>> for KeyframeDto<U> where U: From<T> {
    fn from(v: Keyframe<T>) -> Self { Self { time: v.time.into(), value: v.value.into(), interpolation: v.interpolation.into() } }
}
impl<T, U> From<KeyframeDto<T>> for Keyframe<U> where U: From<T> {
    fn from(v: KeyframeDto<T>) -> Self { Self { time: v.time.into(), value: v.value.into(), interpolation: v.interpolation.into() } }
}
impl From<TransformKeyframes> for TransformKeyframesDto {
    fn from(v: TransformKeyframes) -> Self { Self { position: v.position.into_iter().map(Into::into).collect(), anchor: v.anchor.into_iter().map(Into::into).collect(), scale: v.scale.into_iter().map(Into::into).collect(), rotation_degrees: v.rotation_degrees.into_iter().map(Into::into).collect(), opacity: v.opacity.into_iter().map(Into::into).collect() } }
}
impl From<TransformKeyframesDto> for TransformKeyframes {
    fn from(v: TransformKeyframesDto) -> Self { Self { position: v.position.into_iter().map(Into::into).collect(), anchor: v.anchor.into_iter().map(Into::into).collect(), scale: v.scale.into_iter().map(Into::into).collect(), rotation_degrees: v.rotation_degrees.into_iter().map(Into::into).collect(), opacity: v.opacity.into_iter().map(Into::into).collect() } }
}

impl From<&Project> for ProjectDto {
    fn from(p: &Project) -> Self { Self { id: p.id, name: p.name.clone(), canvas: p.canvas.clone().into(), frame_rate: p.frame_rate.into(), assets: p.assets.clone().into_iter().map(Into::into).collect(), tracks: p.tracks.clone().into_iter().map(Into::into).collect() } }
}
impl From<ProjectDto> for Project {
    fn from(p: ProjectDto) -> Self { Self { id: p.id, name: p.name, canvas: p.canvas.into(), frame_rate: p.frame_rate.into(), assets: p.assets.into_iter().map(Into::into).collect(), tracks: p.tracks.into_iter().map(Into::into).collect() } }
}

#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("project validation failed: {0}")]
    Invalid(#[from] ProjectError),
    #[error("project file I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("project JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported project format {format:?} version {version}")]
    UnsupportedVersion { format: String, version: u32 },
}

/// Validate, write to a unique sibling temporary file, sync, then atomically
/// replace the destination. The parent directory is synced after rename.
pub fn save_atomic(path: impl AsRef<Path>, project: &Project) -> Result<(), PersistenceError> {
    project.validate()?;
    let path = path.as_ref();
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination must name a file"))?;
    let dto = ProjectFileDto { format: "motion-editor.project".into(), version: FORMAT_VERSION, project: serde_json::to_value(ProjectDto::from(project))? };
    let bytes = serde_json::to_vec_pretty(&dto)?;
    let mut temp_path = PathBuf::from(parent);
    let mut temp_name = name.to_os_string();
    temp_name.push(format!(".{}.tmp", uuid::Uuid::new_v4()));
    temp_path.push(temp_name);
    let result = (|| -> Result<(), PersistenceError> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temp_path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp_path, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() { let _ = fs::remove_file(&temp_path); }
    result
}

/// Load and validate a versioned project JSON document.
pub fn load(path: impl AsRef<Path>) -> Result<Project, PersistenceError> {
    let bytes = fs::read(path)?;
    let dto: ProjectFileDto = serde_json::from_slice(&bytes)?;
    if dto.format != "motion-editor.project" || dto.version != FORMAT_VERSION {
        return Err(PersistenceError::UnsupportedVersion { format: dto.format, version: dto.version });
    }
    let project = Project::from(serde_json::from_value::<ProjectDto>(dto.project)?);
    project.validate()?;
    Ok(project)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Canvas, Clip, Interpolation, Keyframe, MediaAsset, MediaKind, StreamMetadata, Track, TrackKind, Transform, TransformKeyframes, Vec2};
    use crate::time::{FrameRate, Tick, TimeBase};

    fn sample() -> Project {
        let asset_id = uuid::Uuid::new_v4();
        Project {
            id: uuid::Uuid::new_v4(), name: "round trip".into(),
            canvas: Canvas { width: 1280, height: 720 },
            frame_rate: FrameRate::new(30, 1).unwrap(),
            assets: vec![MediaAsset { id: asset_id, locator: "image.png".into(), kind: MediaKind::Image, duration: None, streams: vec![StreamMetadata { stream_index: 0, kind: MediaKind::Image, time_base: TimeBase::new(1, 90_000).unwrap(), first_pts: Some(7), duration_pts: None }] }],
            tracks: vec![Track { id: uuid::Uuid::new_v4(), name: "Video".into(), kind: TrackKind::Visual, clips: vec![Clip { id: uuid::Uuid::new_v4(), asset_id, timeline_start: Tick(10), source_in: Tick::ZERO, duration: Tick(500), transform: Transform { position: Vec2 { x: 12.0, y: -3.0 }, ..Transform::default() }, keyframes: TransformKeyframes { opacity: vec![Keyframe { time: Tick(20), value: 0.75, interpolation: Interpolation::EaseInOut }], ..TransformKeyframes::default() } }] }],
        }
    }
    #[test]
    fn save_and_load_round_trip() {
        let path = std::env::temp_dir().join(format!("motion-core-{}.mproj", uuid::Uuid::new_v4()));
        let p = sample(); save_atomic(&path, &p).unwrap();
        assert_eq!(load(&path).unwrap(), p); let _ = fs::remove_file(path);
    }
    #[test]
    fn load_rejects_unknown_version() {
        let path = std::env::temp_dir().join(format!("motion-core-{}.mproj", uuid::Uuid::new_v4()));
        fs::write(&path, r#"{"format":"motion-editor.project","version":99,"project":{}}"#).unwrap();
        assert!(matches!(load(&path), Err(PersistenceError::UnsupportedVersion { version: 99, .. })));
        let _ = fs::remove_file(path);
        let _ = Tick::ZERO;
    }
}

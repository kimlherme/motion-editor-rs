//! Snapshot-based export orchestration.
//!
//! This crate owns scheduling and file finalization only. A real MP4/H.264
//! encoder is deliberately not bundled: callers must supply an approved
//! [`VideoEncoder`] implementation. [`UnavailableVideoEncoder`] is the
//! explicit default when no such backend is available.

use motion_core::{FrameRate, Project, Tick};
use motion_media::{FrameProvider, RgbaFrame};
use motion_render::{render_frame, RenderError};
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
        Arc,
    },
    thread,
};
use thiserror::Error;

const QUEUE_CAPACITY: usize = 3;
static TEMP_ID: AtomicU64 = AtomicU64::new(1);

/// Fixed output profile for the first vertical export slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportProfile {
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
}

impl ExportProfile {
    pub const VERTICAL_1080P30: Self = Self {
        width: 1080,
        height: 1920,
        frame_rate: FrameRate { numerator: 30, denominator: 1 },
    };
}

/// A frozen copy of project state and its checked timeline end.
#[derive(Clone, Debug)]
pub struct ExportSnapshot {
    project: Project,
    duration: Tick,
}

impl ExportSnapshot {
    pub fn capture(project: &Project) -> Result<Self, ExportError> {
        project.validate().map_err(|e| ExportError::InvalidProject(e.to_string()))?;
        let mut end = Tick::ZERO;
        for clip in project.tracks.iter().flat_map(|track| &track.clips) {
            let clip_end = clip.timeline_start.checked_add(clip.duration).ok_or(ExportError::TimeOverflow)?;
            end = end.max(clip_end);
        }
        Ok(Self { project: project.clone(), duration: end })
    }

    pub fn project(&self) -> &Project { &self.project }
    pub fn duration(&self) -> Tick { self.duration }
}

/// Cooperative cancellation shared with the caller while `run_export` runs.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) { self.0.store(true, Ordering::Release); }
    pub fn is_cancelled(&self) -> bool { self.0.load(Ordering::Acquire) }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportProgress {
    /// Frames successfully accepted by the encoder so far.
    pub completed_frames: u64,
    pub total_frames: u64,
}

pub type ProgressCallback = Arc<dyn Fn(ExportProgress) + Send + Sync + 'static>;

pub struct ExportRequest {
    pub output: PathBuf,
    pub profile: ExportProfile,
    pub cancellation: CancellationToken,
    pub progress: Option<ProgressCallback>,
}

impl ExportRequest {
    pub fn vertical_1080p30(output: impl Into<PathBuf>) -> Self {
        Self { output: output.into(), profile: ExportProfile::VERTICAL_1080P30, cancellation: CancellationToken::default(), progress: None }
    }
}

/// Encoder boundary. `begin` receives a same-directory temporary path; the
/// export job publishes it at the requested destination only after `finish`
/// succeeds. The implementation is responsible for writing valid output.
pub trait VideoEncoder: Send + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    fn begin(&mut self, output: &Path, profile: ExportProfile) -> Result<(), Self::Error>;
    fn encode_frame(&mut self, index: u64, timestamp: Tick, frame: &RgbaFrame) -> Result<(), Self::Error>;
    fn finish(&mut self) -> Result<(), Self::Error>;
}

/// Backend placeholder. No MP4 or H.264 output is produced by this adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableVideoEncoder;

#[derive(Debug, Error)]
#[error("no approved video encoding backend is available (MP4/H.264 export is unavailable)")]
pub struct EncoderUnavailable;

impl VideoEncoder for UnavailableVideoEncoder {
    type Error = EncoderUnavailable;
    fn begin(&mut self, _output: &Path, _profile: ExportProfile) -> Result<(), Self::Error> { Err(EncoderUnavailable) }
    fn encode_frame(&mut self, _index: u64, _timestamp: Tick, _frame: &RgbaFrame) -> Result<(), Self::Error> { Err(EncoderUnavailable) }
    fn finish(&mut self) -> Result<(), Self::Error> { Err(EncoderUnavailable) }
}

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("invalid project: {0}")]
    InvalidProject(String),
    #[error("project duration cannot be represented")]
    TimeOverflow,
    #[error("export profile must be 1080x1920 at 30 fps")]
    UnsupportedProfile,
    #[error("project canvas {project_width}x{project_height} does not match output profile {output_width}x{output_height}")]
    CanvasMismatch { project_width: u32, project_height: u32, output_width: u32, output_height: u32 },
    #[error("export cancelled")]
    Cancelled,
    #[error("frame rendering failed: {0}")]
    Render(#[from] RenderError),
    #[error("encoder failed: {0}")]
    Encoder(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("encoder worker stopped unexpectedly")]
    WorkerStopped,
}

/// Render frames at exact project-clock positions and send them through a
/// bounded queue to the supplied encoder. The destination appears only after
/// the encoder finishes and the temporary file is atomically renamed.
pub fn run_export<P, E>(
    snapshot: &ExportSnapshot,
    request: &ExportRequest,
    provider: &P,
    mut encoder: E,
) -> Result<(), ExportError>
where
    P: FrameProvider,
    E: VideoEncoder,
{
    let profile = request.profile;
    if profile != ExportProfile::VERTICAL_1080P30 { return Err(ExportError::UnsupportedProfile); }
    let project = &snapshot.project;
    if project.canvas.width != profile.width || project.canvas.height != profile.height {
        return Err(ExportError::CanvasMismatch {
            project_width: project.canvas.width, project_height: project.canvas.height,
            output_width: profile.width, output_height: profile.height,
        });
    }
    let output = &request.output;
    let parent = output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temp = temporary_path(output);
    // Reserve a unique same-filesystem name before handing it to the encoder.
    OpenOptions::new().write(true).create_new(true).open(&temp)?;

    let result = (|| {
        let frame_duration = profile.frame_rate.unit_duration().ok_or(ExportError::TimeOverflow)?;
        let total_frames = if snapshot.duration.0 == 0 { 0 } else {
            let numerator = i128::from(snapshot.duration.0) + i128::from(frame_duration.0) - 1;
            u64::try_from(numerator / i128::from(frame_duration.0)).map_err(|_| ExportError::TimeOverflow)?
        };
        let (tx, rx) = mpsc::sync_channel::<(u64, Tick, RgbaFrame)>(QUEUE_CAPACITY);
        let worker_temp = temp.clone();
        let progress = request.progress.clone();
        let worker = thread::spawn(move || -> Result<(), String> {
            encoder.begin(&worker_temp, profile).map_err(|e| e.to_string())?;
            let mut completed = 0_u64;
            while let Ok((index, timestamp, frame)) = rx.recv() {
                encoder.encode_frame(index, timestamp, &frame).map_err(|e| e.to_string())?;
                completed += 1;
                if let Some(callback) = &progress { callback(ExportProgress { completed_frames: completed, total_frames }); }
            }
            encoder.finish().map_err(|e| e.to_string())
        });

        let produce_result = (|| {
            for index in 0..total_frames {
                if request.cancellation.is_cancelled() { return Err(ExportError::Cancelled); }
                let timestamp = Tick::from_rate_units(i64::try_from(index).map_err(|_| ExportError::TimeOverflow)?, profile.frame_rate).ok_or(ExportError::TimeOverflow)?;
                let frame = render_frame(project, timestamp, provider)?;
                if frame.width != profile.width || frame.height != profile.height {
                    return Err(ExportError::CanvasMismatch {
                        project_width: frame.width, project_height: frame.height,
                        output_width: profile.width, output_height: profile.height,
                    });
                }
                tx.send((index, timestamp, frame)).map_err(|_| ExportError::WorkerStopped)?;
            }
            Ok(())
        })();
        drop(tx);
        let worker_result = worker.join().map_err(|_| ExportError::WorkerStopped)?;
        worker_result.map_err(ExportError::Encoder)?;
        produce_result?;
        if request.cancellation.is_cancelled() { return Err(ExportError::Cancelled); }
        // Ensure the completed artifact is flushed before the atomic publish.
        OpenOptions::new().read(true).write(true).open(&temp)?.sync_all()?;
        fs::rename(&temp, output)?;
        if let Ok(dir) = OpenOptions::new().read(true).open(parent) { let _ = dir.sync_all(); }
        Ok(())
    })();

    if result.is_err() { let _ = fs::remove_file(&temp); }
    result
}

fn temporary_path(output: &Path) -> PathBuf {
    let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let name = output.file_name().unwrap_or_default().to_string_lossy();
    output.with_file_name(format!(".{name}.motion-export-{}-{id}.tmp", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use motion_core::{Canvas, Clip, MediaAsset, MediaKind, Track, TrackKind, Transform, TransformKeyframes};
    use std::sync::{Arc, Mutex};
    use motion_core::project::StableId;

    fn vertical_project() -> Project {
        let asset_id = StableId::new_v4();
        Project { id: StableId::new_v4(), name: "export test".into(), canvas: Canvas { width: 1080, height: 1920 }, frame_rate: FrameRate::new(30, 1).unwrap(),
            assets: vec![MediaAsset { id: asset_id, locator: "still.png".into(), kind: MediaKind::Image, duration: None, streams: vec![] }],
            tracks: vec![Track { id: StableId::new_v4(), name: "visual".into(), kind: TrackKind::Visual, clips: vec![Clip { id: StableId::new_v4(), asset_id, timeline_start: Tick::ZERO, source_in: Tick::ZERO, duration: Tick(16_934_400_000), transform: Transform::default(), keyframes: TransformKeyframes::default() }] }] }
    }

    #[derive(Clone, Copy)] struct BlankProvider;
    impl FrameProvider for BlankProvider {
        fn frame_at(&self, _asset: &MediaAsset, _source_time: Tick) -> Result<Option<motion_media::ProvidedFrame>, motion_media::MediaError> { Ok(None) }
    }

    #[derive(Debug, Error)] #[error("test encoder failure")] struct TestEncoderError;
    struct CountingEncoder(Arc<Mutex<Vec<(u64, Tick)>>>);
    impl VideoEncoder for CountingEncoder {
        type Error = TestEncoderError;
        fn begin(&mut self, output: &Path, _profile: ExportProfile) -> Result<(), Self::Error> { fs::write(output, b"encoded").unwrap(); Ok(()) }
        fn encode_frame(&mut self, index: u64, time: Tick, _frame: &RgbaFrame) -> Result<(), Self::Error> { self.0.lock().unwrap().push((index, time)); Ok(()) }
        fn finish(&mut self) -> Result<(), Self::Error> { Ok(()) }
    }

    #[test]
    fn frozen_snapshot_exports_exact_frame_ticks_and_publishes_only_finished_file() {
        let source = vertical_project();
        let snapshot = ExportSnapshot::capture(&source).unwrap();
        let mut changed = source.clone(); changed.name = "later edit".into();
        assert_eq!(changed.name, "later edit");
        assert_eq!(snapshot.project().name, "export test");
        let path = std::env::temp_dir().join(format!("motion-export-test-{}.bin", StableId::new_v4()));
        let encoded = Arc::new(Mutex::new(Vec::new()));
        let request = ExportRequest::vertical_1080p30(&path);
        run_export(&snapshot, &request, &BlankProvider, CountingEncoder(encoded.clone())).unwrap();
        assert_eq!(*encoded.lock().unwrap(), vec![(0, Tick::ZERO), (1, Tick(8_467_200_000))]);
        assert_eq!(fs::read(&path).unwrap(), b"encoded");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn cancelled_job_does_not_publish_destination() {
        let snapshot = ExportSnapshot::capture(&vertical_project()).unwrap();
        let path = std::env::temp_dir().join(format!("motion-export-cancel-{}.bin", StableId::new_v4()));
        let request = ExportRequest::vertical_1080p30(&path);
        request.cancellation.cancel();
        let result = run_export(&snapshot, &request, &BlankProvider, CountingEncoder(Arc::new(Mutex::new(Vec::new()))));
        assert!(matches!(result, Err(ExportError::Cancelled)));
        assert!(!path.exists());
    }

    #[test]
    fn unavailable_backend_fails_without_claiming_or_publishing_output() {
        let snapshot = ExportSnapshot::capture(&vertical_project()).unwrap();
        let path = std::env::temp_dir().join(format!("motion-export-unavailable-{}.mp4", StableId::new_v4()));
        let result = run_export(&snapshot, &ExportRequest::vertical_1080p30(&path), &BlankProvider, UnavailableVideoEncoder);
        assert!(matches!(result, Err(ExportError::Encoder(_))));
        assert!(!path.exists());
    }
}

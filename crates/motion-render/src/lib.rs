//! CPU reference compositor shared by preview and export.
//!
//! Layers are composited in track vector order (bottom to top). Positions are
//! measured from the canvas center, with positive Y pointing down. An anchor is
//! a normalized point in the source image (`[0, 0]` is top-left and `[1, 1]` is
//! bottom-right); the default `[0.5, 0.5]` keeps the image center at the canvas
//! position.

use motion_animation::evaluate_transform;
use motion_core::{
    project::{MediaKind, Project, ProjectError, TrackKind},
    time::Tick,
};
use motion_media::{FrameProvider, MediaError, RgbaFrame};
use thiserror::Error;

/// Maximum output allocation (256 MiB, including RGBA bytes).
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024 * 1024;
/// Bound dimensions before calculating output or raster coordinates.
pub const MAX_CANVAS_DIMENSION: u32 = 16_384;
/// Bound work for malformed projects with enormous clip lists.
pub const MAX_COMPOSITED_CLIPS: usize = 4_096;

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("invalid project: {0}")]
    InvalidProject(#[from] ProjectError),
    #[error("media provider failed: {0}")]
    Media(#[from] MediaError),
    #[error("canvas dimensions exceed compositor limits")]
    CanvasTooLarge,
    #[error("project has too many clips for one frame")]
    TooManyClips,
    #[error("media frame has invalid dimensions or pixel buffer")]
    InvalidMediaFrame,
    #[error("could not allocate the output frame")]
    AllocationFailed,
}

/// Render one sequence-time frame to a transparent RGBA8 canvas.
///
/// Clips use the half-open timeline interval `[timeline_start,
/// timeline_start + duration)`. Source time is `source_in + elapsed`. Missing
/// provider frames are treated as transparent for that clip, which lets a
/// decoder report a temporary gap without displaying an unrelated old frame.
pub fn render_frame<P: FrameProvider>(
    project: &Project,
    sequence_time: Tick,
    frame_provider: &P,
) -> Result<RgbaFrame, RenderError> {
    project.validate()?;
    let width = project.canvas.width;
    let height = project.canvas.height;
    let pixels = checked_frame_len(width, height).ok_or(RenderError::CanvasTooLarge)?;
    if width > MAX_CANVAS_DIMENSION || height > MAX_CANVAS_DIMENSION || pixels > MAX_OUTPUT_BYTES {
        return Err(RenderError::CanvasTooLarge);
    }
    let mut output = Vec::new();
    output.try_reserve_exact(pixels).map_err(|_| RenderError::AllocationFailed)?;
    output.resize(pixels, 0);

    let clip_count = project.tracks.iter().filter(|track| track.kind == TrackKind::Visual)
        .map(|track| track.clips.len()).try_fold(0usize, usize::checked_add)
        .ok_or(RenderError::TooManyClips)?;
    if clip_count > MAX_COMPOSITED_CLIPS {
        return Err(RenderError::TooManyClips);
    }

    // Track order is the stable bottom-to-top stacking order. Clip order inside
    // a track is likewise preserved for deterministic overlap behavior.
    for track in project.tracks.iter().filter(|track| track.kind == TrackKind::Visual) {
        for clip in &track.clips {
            let Some(timeline_end) = clip.timeline_start.checked_add(clip.duration) else { continue };
            if sequence_time < clip.timeline_start || sequence_time >= timeline_end {
                continue;
            }
            let Some(source_time) = clip.source_in.checked_add(Tick(sequence_time.0 - clip.timeline_start.0)) else { continue };
            let Some(asset) = project.assets.iter().find(|asset| asset.id == clip.asset_id) else { continue };
            if asset.kind == MediaKind::Audio { continue; }
            let Some(provided) = frame_provider.frame_at(asset, source_time)? else { continue };
            validate_source_frame(&provided.frame)?;

            let transform = evaluate_transform(clip, source_time);
            composite_layer(
                &mut output, width, height, &provided.frame, transform.position,
                transform.anchor, transform.scale, transform.rotation_degrees, transform.opacity,
            );
        }
    }

    RgbaFrame::new(width, height, output).map_err(|_| RenderError::AllocationFailed)
}

fn checked_frame_len(width: u32, height: u32) -> Option<usize> {
    usize::try_from(width).ok()?.checked_mul(usize::try_from(height).ok()?)?.checked_mul(4)
}

fn validate_source_frame(frame: &RgbaFrame) -> Result<(), RenderError> {
    if frame.width == 0 || frame.height == 0
        || frame.width > MAX_CANVAS_DIMENSION || frame.height > MAX_CANVAS_DIMENSION
        || checked_frame_len(frame.width, frame.height) != Some(frame.pixels().len())
        || frame.pixels().len() > MAX_OUTPUT_BYTES
    {
        return Err(RenderError::InvalidMediaFrame);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn composite_layer(
    dst: &mut [u8], canvas_width: u32, canvas_height: u32, src: &RgbaFrame,
    position: motion_core::project::Vec2, anchor: motion_core::project::Vec2,
    scale: motion_core::project::Vec2, rotation_degrees: f64, opacity: f64,
) {
    if !position.x.is_finite() || !position.y.is_finite()
        || !anchor.x.is_finite() || !anchor.y.is_finite()
        || !scale.x.is_finite() || !scale.y.is_finite()
        || !rotation_degrees.is_finite() || !opacity.is_finite()
        || opacity <= 0.0 || scale.x == 0.0 || scale.y == 0.0
    { return; }

    let radians = rotation_degrees.to_radians();
    let (sin, cos) = radians.sin_cos();
    let sw = f64::from(src.width);
    let sh = f64::from(src.height);
    let cw = f64::from(canvas_width);
    let ch = f64::from(canvas_height);
    let center_x = cw * 0.5 + position.x;
    let center_y = ch * 0.5 + position.y;

    // Transform all source corners to find a clipped destination bounding box.
    let corners = [(0.0, 0.0), (sw, 0.0), (0.0, sh), (sw, sh)];
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for (x, y) in corners {
        let (tx, ty) = forward(x, y, sw, sh, anchor.x, anchor.y, scale.x, scale.y, cos, sin);
        min_x = min_x.min(center_x + tx);
        max_x = max_x.max(center_x + tx);
        min_y = min_y.min(center_y + ty);
        max_y = max_y.max(center_y + ty);
    }
    if !min_x.is_finite() || !max_x.is_finite() || !min_y.is_finite() || !max_y.is_finite()
        || max_x <= 0.0 || max_y <= 0.0 || min_x >= cw || min_y >= ch
    { return; }

    let x0 = min_x.floor().max(0.0).min(cw) as u32;
    let x1 = max_x.ceil().max(0.0).min(cw) as u32;
    let y0 = min_y.floor().max(0.0).min(ch) as u32;
    let y1 = max_y.ceil().max(0.0).min(ch) as u32;
    let det = scale.x * scale.y;
    if !det.is_finite() || det == 0.0 { return; }
    let effective_opacity = opacity.clamp(0.0, 1.0);
    let source = src.pixels();

    // Inverse-map destination pixel centers, then nearest-neighbor sample.
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = f64::from(x) + 0.5 - center_x;
            let dy = f64::from(y) + 0.5 - center_y;
            let rx = cos * dx + sin * dy;
            let ry = -sin * dx + cos * dy;
            let sx = rx / scale.x + sw * anchor.x;
            let sy = ry / scale.y + sh * anchor.y;
            if sx < 0.0 || sy < 0.0 || sx >= sw || sy >= sh { continue; }
            let sx = sx.floor() as u32;
            let sy = sy.floor() as u32;
            let src_i = ((sy as usize * src.width as usize) + sx as usize) * 4;
            let dst_i = ((y as usize * canvas_width as usize) + x as usize) * 4;
            blend_over(dst, dst_i, source, src_i, effective_opacity);
        }
    }
}

fn forward(x: f64, y: f64, sw: f64, sh: f64, ax: f64, ay: f64,
    scale_x: f64, scale_y: f64, cos: f64, sin: f64) -> (f64, f64) {
    let lx = (x - sw * ax) * scale_x;
    let ly = (y - sh * ay) * scale_y;
    (cos * lx - sin * ly, sin * lx + cos * ly)
}

fn blend_over(dst: &mut [u8], di: usize, src: &[u8], si: usize, opacity: f64) {
    let source_alpha = f64::from(src[si + 3]) / 255.0 * opacity;
    if source_alpha <= 0.0 { return; }
    let dest_alpha = f64::from(dst[di + 3]) / 255.0;
    let out_alpha = source_alpha + dest_alpha * (1.0 - source_alpha);
    if out_alpha <= 0.0 { return; }
    for channel in 0..3 {
        let source = f64::from(src[si + channel]) / 255.0;
        let dest = f64::from(dst[di + channel]) / 255.0;
        let out = (source * source_alpha + dest * dest_alpha * (1.0 - source_alpha)) / out_alpha;
        dst[di + channel] = (out * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    dst[di + 3] = (out_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
}


#[cfg(test)]
mod tests {
    use super::*;
    use motion_core::{
        project::{Canvas, Clip, Keyframe, MediaAsset, Track, Transform, TransformKeyframes, Vec2, Interpolation},
        time::FrameRate,
    };
    use motion_media::{MediaTimestamp, Rational, ProvidedFrame};
    use uuid::Uuid;

    struct SolidProvider;
    impl FrameProvider for SolidProvider {
        fn frame_at(&self, _asset: &motion_core::project::MediaAsset, _source_time: Tick)
            -> Result<Option<ProvidedFrame>, MediaError> {
            Ok(Some(ProvidedFrame {
                frame: RgbaFrame::new(1, 1, vec![255, 0, 0, 255])?,
                timestamp: MediaTimestamp { pts: 0, time_base: Rational::new(1, 1)? },
            }))
        }
    }

    struct SquareProvider;
    impl FrameProvider for SquareProvider {
        fn frame_at(&self, _asset: &motion_core::project::MediaAsset, _source_time: Tick)
            -> Result<Option<ProvidedFrame>, MediaError> {
            Ok(Some(ProvidedFrame {
                frame: RgbaFrame::new(2, 2, vec![255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255])?,
                timestamp: MediaTimestamp { pts: 0, time_base: Rational::new(1, 1)? },
            }))
        }
    }

    struct TimestampColorProvider;
    impl FrameProvider for TimestampColorProvider {
        fn frame_at(&self, _asset: &motion_core::project::MediaAsset, source_time: Tick)
            -> Result<Option<ProvidedFrame>, MediaError> {
            let color = if source_time == Tick(7) { [255, 0, 0, 255] } else { [0, 0, 255, 255] };
            Ok(Some(ProvidedFrame {
                frame: RgbaFrame::new(1, 1, color.to_vec())?,
                timestamp: MediaTimestamp { pts: source_time.0, time_base: Rational::new(1, 1)? },
            }))
        }
    }

    fn fixture() -> (Project, Uuid) {
        let asset_id = Uuid::new_v4();
        let asset = MediaAsset {
            id: asset_id,
            locator: "fixture.png".into(),
            kind: MediaKind::Image,
            duration: None,
            streams: vec![],
        };
        let clip = Clip {
            id: Uuid::new_v4(), asset_id, timeline_start: Tick(10),
            source_in: Tick(7), duration: Tick(5), transform: Transform::default(),
            keyframes: Default::default(),
        };
        (Project {
            id: Uuid::new_v4(), name: "render test".into(),
            canvas: Canvas { width: 3, height: 3 },
            frame_rate: FrameRate::new(30, 1).unwrap(),
            assets: vec![asset],
            tracks: vec![Track { id: Uuid::new_v4(), name: "visual".into(), kind: TrackKind::Visual, clips: vec![clip] }],
        }, asset_id)
    }

    #[test]
    fn draws_a_centered_source_only_inside_the_half_open_clip_interval() {
        let (project, _) = fixture();
        let before = render_frame(&project, Tick(9), &SolidProvider).unwrap();
        assert!(before.pixels().iter().all(|byte| *byte == 0));
        let active = render_frame(&project, Tick(10), &SolidProvider).unwrap();
        assert_eq!(active.pixels(), &[0,0,0,0, 0,0,0,0, 0,0,0,0,
                                      0,0,0,0, 255,0,0,255, 0,0,0,0,
                                      0,0,0,0, 0,0,0,0, 0,0,0,0]);
        let at_end = render_frame(&project, Tick(15), &SolidProvider).unwrap();
        assert!(at_end.pixels().iter().all(|byte| *byte == 0));
    }

    #[test]
    fn normalized_anchor_controls_the_source_point_pinned_to_canvas_center() {
        let (mut project, _) = fixture();
        let centered = render_frame(&project, Tick(10), &SquareProvider).unwrap();
        assert_eq!(&centered.pixels()[0..4], &[255, 0, 0, 255]);

        project.tracks[0].clips[0].transform.anchor = Vec2 { x: 0.0, y: 0.0 };
        let top_left_pinned = render_frame(&project, Tick(10), &SquareProvider).unwrap();
        assert_eq!(&top_left_pinned.pixels()[16..20], &[255, 0, 0, 255]);
        assert_eq!(&top_left_pinned.pixels()[0..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn later_visual_tracks_composite_over_earlier_tracks() {
        let (mut project, asset_id) = fixture();
        let upper_clip = Clip {
            id: Uuid::new_v4(), asset_id, timeline_start: Tick(10),
            source_in: Tick(20), duration: Tick(5), transform: Transform::default(),
            keyframes: Default::default(),
        };
        project.tracks.push(Track {
            id: Uuid::new_v4(), name: "upper".into(), kind: TrackKind::Visual,
            clips: vec![upper_clip],
        });
        let frame = render_frame(&project, Tick(10), &TimestampColorProvider).unwrap();
        assert_eq!(&frame.pixels()[16..20], &[0, 0, 255, 255]);
    }

    #[test]
    fn rejects_invalid_or_oversized_canvas_without_panicking() {
        let (mut project, _) = fixture();
        project.canvas.width = 0;
        assert!(matches!(render_frame(&project, Tick(0), &SolidProvider), Err(RenderError::InvalidProject(_))));
        project.canvas.width = MAX_CANVAS_DIMENSION + 1;
        assert!(matches!(render_frame(&project, Tick(0), &SolidProvider), Err(RenderError::CanvasTooLarge)));
    }

    #[test]
    fn applies_clip_position_and_evaluated_opacity() {
        let (mut project, _) = fixture();
        let clip = &mut project.tracks[0].clips[0];
        clip.transform.position = Vec2 { x: 1.0, y: 0.0 };
        clip.keyframes = TransformKeyframes {
            opacity: vec![Keyframe { time: Tick(7), value: 0.5, interpolation: Interpolation::Linear }],
            ..Default::default()
        };
        let frame = render_frame(&project, Tick(10), &SolidProvider).unwrap();
        assert_eq!(&frame.pixels()[20..24], &[255, 0, 0, 128]);
        assert_eq!(&frame.pixels()[16..20], &[0, 0, 0, 0]);
    }

    #[test]
    fn alpha_blending_applies_layer_opacity() {
        let mut dst = [0, 0, 255, 255];
        blend_over(&mut dst, 0, &[255, 0, 0, 255], 0, 0.5);
        assert_eq!(dst, [128, 0, 128, 255]);
    }

}

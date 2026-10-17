//! Bounded still-image import and media source contracts.
//!
//! This crate deliberately does not decode video or audio. A codec backend must
//! be selected and reviewed before those provider contracts can be implemented.

use motion_core::{MediaAsset, MediaKind, Tick, TICKS_PER_SECOND, project::StableId};
use std::{path::Path, sync::Arc};
use thiserror::Error;

/// Largest accepted width or height for a decoded raster image.
pub const MAX_IMAGE_DIMENSION: u32 = 16_384;
/// Largest decoded RGBA8 buffer accepted by this crate (256 MiB).
pub const MAX_RGBA_BYTES: usize = 256 * 1024 * 1024;

/// A validated rational number. For stream time bases, this is seconds per PTS
/// unit: `numerator / denominator`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Rational {
    pub numerator: i64,
    pub denominator: i64,
}

impl Rational {
    pub fn new(numerator: i64, denominator: i64) -> Result<Self, MediaError> {
        if numerator <= 0 || denominator <= 0 {
            return Err(MediaError::InvalidTimeBase { numerator, denominator });
        }
        Ok(Self { numerator, denominator })
    }
}

/// Original timestamp and time base reported for one media stream.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MediaTimestamp {
    pub pts: i64,
    pub time_base: Rational,
}

impl MediaTimestamp {
    /// Convert to project ticks only when the result is exactly representable.
    /// The original `pts` and `time_base` remain available to callers.
    pub fn to_tick(self) -> Result<Tick, MediaError> {
        if self.time_base.numerator <= 0 || self.time_base.denominator <= 0 {
            return Err(MediaError::InvalidTimeBase {
                numerator: self.time_base.numerator,
                denominator: self.time_base.denominator,
            });
        }
        let scaled = i128::from(self.pts)
            .checked_mul(i128::from(self.time_base.numerator))
            .and_then(|v| v.checked_mul(i128::from(TICKS_PER_SECOND)))
            .ok_or(MediaError::TimestampOverflow)?;
        let denominator = i128::from(self.time_base.denominator);
        if scaled % denominator != 0 {
            return Err(MediaError::InexactTimestamp);
        }
        i64::try_from(scaled / denominator)
            .map(Tick)
            .map_err(|_| MediaError::TimestampOverflow)
    }
}

/// Immutable RGBA8 image buffer. Construction validates dimensions and byte
/// count before the buffer can reach the renderer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    pixels: Arc<[u8]>,
}

impl RgbaFrame {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, MediaError> {
        let expected = checked_rgba_len(width, height)?;
        if pixels.len() != expected {
            return Err(MediaError::InvalidBufferLength {
                expected,
                actual: pixels.len(),
            });
        }
        Ok(Self {
            width,
            height,
            pixels: Arc::from(pixels),
        })
    }

    /// Byte order is R, G, B, A for each pixel; the returned slice is read-only.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

fn checked_rgba_len(width: u32, height: u32) -> Result<usize, MediaError> {
    if width == 0 || height == 0 || width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return Err(MediaError::InvalidDimensions { width, height });
    }
    let len = usize::try_from(width)
        .ok()
        .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(MediaError::AllocationLimit)?;
    if len > MAX_RGBA_BYTES {
        return Err(MediaError::AllocationLimit);
    }
    Ok(len)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvidedFrame {
    pub frame: RgbaFrame,
    pub timestamp: MediaTimestamp,
}

/// Supplies one imported still image at every requested source time. Its
/// synthetic timestamp is expressed in project ticks, so the still's clock
/// follows the sequence time while preserving the provider timestamp contract.
#[derive(Clone, Debug)]
pub struct StillImageProvider {
    asset_id: StableId,
    frame: RgbaFrame,
}

impl StillImageProvider {
    pub fn new(asset_id: StableId, frame: RgbaFrame) -> Self {
        Self { asset_id, frame }
    }
}

impl FrameProvider for StillImageProvider {
    fn frame_at(
        &self,
        asset: &MediaAsset,
        source_time: Tick,
    ) -> Result<Option<ProvidedFrame>, MediaError> {
        if asset.kind != MediaKind::Image {
            return Err(MediaError::UnsupportedBackend(
                "video frame decoding (no video backend is configured)",
            ));
        }
        if asset.id != self.asset_id {
            return Err(MediaError::AssetNotRegistered(asset.id));
        }
        Ok(Some(ProvidedFrame {
            frame: self.frame.clone(),
            timestamp: MediaTimestamp {
                pts: source_time.0,
                time_base: Rational {
                    numerator: 1,
                    denominator: TICKS_PER_SECOND,
                },
            },
        }))
    }
}

/// Provides the decoded video/image frame corresponding to source time.
/// Implementations return `None` when no frame exists at that source time.
pub trait FrameProvider: Send + Sync {
    fn frame_at(
        &self,
        asset: &MediaAsset,
        source_time: Tick,
    ) -> Result<Option<ProvidedFrame>, MediaError>;
}

/// Audio samples retain the timestamp and rational time base of their stream.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioChunk {
    pub samples: Arc<[f32]>,
    pub channels: u16,
    pub sample_rate: u32,
    pub timestamp: MediaTimestamp,
}

/// Audio source boundary. No implementation is provided without an approved
/// decode backend.
pub trait AudioProvider: Send + Sync {
    fn audio_at(
        &self,
        asset: &MediaAsset,
        source_time: Tick,
        max_samples_per_channel: usize,
    ) -> Result<Option<AudioChunk>, MediaError>;
}

/// Explicit placeholder for builds that have no approved video decoder.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnsupportedVideoProvider;

impl FrameProvider for UnsupportedVideoProvider {
    fn frame_at(
        &self,
        _asset: &MediaAsset,
        _source_time: Tick,
    ) -> Result<Option<ProvidedFrame>, MediaError> {
        Err(MediaError::UnsupportedBackend("video decoding"))
    }
}

/// Explicit placeholder for builds that have no approved audio decoder.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnsupportedAudioProvider;

impl AudioProvider for UnsupportedAudioProvider {
    fn audio_at(
        &self,
        _asset: &MediaAsset,
        _source_time: Tick,
        _max_samples_per_channel: usize,
    ) -> Result<Option<AudioChunk>, MediaError> {
        Err(MediaError::UnsupportedBackend("audio decoding"))
    }
}

/// Decode a local PNG or JPEG still image using the workspace's `image`
/// dependency and explicit decoder allocation limits.
pub fn load_still(path: impl AsRef<Path>) -> Result<RgbaFrame, MediaError> {
    let mut reader = still_reader(path)?;
    let decoded = reader.decode()?;
    let rgba = decoded.into_rgba8();
    let (width, height) = rgba.dimensions();
    // Revalidate dimensions and size after decode as defense in depth.
    checked_rgba_len(width, height)?;
    RgbaFrame::new(width, height, rgba.into_raw())
}

/// Read bounded PNG/JPEG dimensions without materializing the full pixel buffer.
pub fn probe_still(path: impl AsRef<Path>) -> Result<StillImageInfo, MediaError> {
    let reader = still_reader(path)?;
    let format = reader.format().ok_or(MediaError::UnknownImageFormat)?;
    let (width, height) = reader.into_dimensions()?;
    checked_rgba_len(width, height)?;
    Ok(StillImageInfo {
        width,
        height,
        format: match format {
            image::ImageFormat::Png => StillImageFormat::Png,
            image::ImageFormat::Jpeg => StillImageFormat::Jpeg,
            _ => return Err(MediaError::UnsupportedImageFormat),
        },
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StillImageInfo {
    pub width: u32,
    pub height: u32,
    pub format: StillImageFormat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StillImageFormat {
    Png,
    Jpeg,
}

fn still_reader(path: impl AsRef<Path>) -> Result<image::ImageReader, MediaError> {
    let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
    match reader.format() {
        Some(image::ImageFormat::Png | image::ImageFormat::Jpeg) => {}
        Some(_) => return Err(MediaError::UnsupportedImageFormat),
        None => return Err(MediaError::UnknownImageFormat),
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_RGBA_BYTES as u64);
    reader.limits(limits);
    Ok(reader)
}

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("asset {0} is not registered with this media provider")]
    AssetNotRegistered(StableId),
    #[error("unsupported backend: {0}")]
    UnsupportedBackend(&'static str),
    #[error("only PNG and JPEG still images are supported")]
    UnsupportedImageFormat,
    #[error("image format could not be identified")]
    UnknownImageFormat,
    #[error("invalid image dimensions {width}x{height}")]
    InvalidDimensions { width: u32, height: u32 },
    #[error("RGBA allocation exceeds the configured limit")]
    AllocationLimit,
    #[error("RGBA buffer has {actual} bytes; expected {expected}")]
    InvalidBufferLength { expected: usize, actual: usize },
    #[error("invalid stream time base {numerator}/{denominator}")]
    InvalidTimeBase { numerator: i64, denominator: i64 },
    #[error("media timestamp cannot be represented in project ticks")]
    TimestampOverflow,
    #[error("media timestamp does not map to an integral project tick")]
    InexactTimestamp,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Image(#[from] image::ImageError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_frame_checks_byte_count_and_dimensions() {
        let frame = RgbaFrame::new(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert_eq!(frame.pixels(), &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(matches!(
            RgbaFrame::new(2, 1, vec![0; 7]),
            Err(MediaError::InvalidBufferLength { .. })
        ));
        assert!(matches!(
            RgbaFrame::new(MAX_IMAGE_DIMENSION + 1, 1, vec![]),
            Err(MediaError::InvalidDimensions { .. })
        ));
        assert!(matches!(RgbaFrame::new(0, 1, vec![]), Err(MediaError::InvalidDimensions { .. })));
    }

    #[test]
    fn rational_stream_time_base_converts_without_losing_pts() {
        let stamp = MediaTimestamp {
            pts: 48_000,
            time_base: Rational::new(1, 48_000).unwrap(),
        };
        assert!(matches!(stamp.to_tick(), Ok(tick) if tick == Tick(TICKS_PER_SECOND)));
        assert_eq!(stamp.pts, 48_000);
        assert_eq!(stamp.time_base.denominator, 48_000);
        assert!(Rational::new(1, 0).is_err());
    }

    #[test]
    fn timestamp_conversion_rejects_fractional_ticks_and_overflow() {
        let fractional = MediaTimestamp {
            pts: 1,
            time_base: Rational::new(1, i64::MAX).unwrap(),
        };
        assert!(matches!(fractional.to_tick(), Err(MediaError::InexactTimestamp)));
        let overflow = MediaTimestamp {
            pts: i64::MAX,
            time_base: Rational::new(i64::MAX, 1).unwrap(),
        };
        assert!(matches!(overflow.to_tick(), Err(MediaError::TimestampOverflow)));
    }

    #[test]
    fn missing_codec_backend_has_an_explicit_error() {
        let error = MediaError::UnsupportedBackend("video decoding");
        assert_eq!(error.to_string(), "unsupported backend: video decoding");
    }

    #[test]
    fn still_provider_uses_a_synthetic_project_clock_and_rejects_video() {
        let asset = MediaAsset {
            id: motion_core::project::StableId::from_u128(1),
            locator: "still.png".into(),
            kind: MediaKind::Image,
            duration: None,
            streams: vec![],
        };
        let provider = StillImageProvider::new(
            asset.id,
            RgbaFrame::new(1, 1, vec![1, 2, 3, 255]).unwrap(),
        );
        let provided = provider.frame_at(&asset, Tick(17)).unwrap().unwrap();
        assert_eq!(provided.timestamp.pts, 17);
        assert_eq!(provided.timestamp.to_tick().unwrap(), Tick(17));

        let unregistered_image = MediaAsset {
            id: motion_core::project::StableId::from_u128(2),
            ..asset.clone()
        };
        assert!(matches!(
            provider.frame_at(&unregistered_image, Tick(17)),
            Err(MediaError::AssetNotRegistered(id)) if id == unregistered_image.id
        ));

        let video_asset = MediaAsset {
            kind: MediaKind::Video,
            ..asset
        };
        assert!(matches!(
            provider.frame_at(&video_asset, Tick(17)),
            Err(MediaError::UnsupportedBackend(_))
        ));
    }
}

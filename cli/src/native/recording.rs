use serde_json::{json, Value};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

use super::cdp::client::CdpClient;
use super::cdp::types::{CaptureScreenshotParams, CaptureScreenshotResult};

const CAPTURE_INTERVAL_MS: u64 = 100;
const CAPTURE_FPS: u32 = 10;

/// `record start <path>.gif` (#781): a shareable animated GIF, encoded in
/// process (no ffmpeg). 5 frames a second and at most 800 px wide keep a
/// minute of browsing to a few MB; unchanged frames are merged into one
/// longer frame.
const GIF_INTERVAL_MS: u64 = 200;
const GIF_MAX_WIDTH: u32 = 800;
/// NeuQuant speed for the palette, 1 (best) to 30 (fastest). 10 is the
/// quality/CPU balance the `gif` crate recommends.
const GIF_QUANT_SPEED: i32 = 10;
/// Frames waiting for the encoder; a frame captured while it is behind is
/// dropped (its time goes to the frame before it) rather than queued without
/// bound.
const GIF_QUEUE: usize = 8;

/// Whether a recording to `path` is written as an animated GIF.
pub fn is_gif_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gif"))
}

pub struct RecordingState {
    pub active: bool,
    pub output_path: String,
    pub frame_count: u64,
    pub capture_task: Option<tokio::task::JoinHandle<Result<(), String>>>,
    pub shared_frame_count: Option<Arc<AtomicU64>>,
    pub cancel_tx: Option<oneshot::Sender<()>>,
}

impl RecordingState {
    pub fn new() -> Self {
        Self {
            active: false,
            output_path: String::new(),
            frame_count: 0,
            capture_task: None,
            shared_frame_count: None,
            cancel_tx: None,
        }
    }
}

pub fn recording_start(state: &mut RecordingState, path: &str) -> Result<Value, String> {
    if state.active {
        return Err("Recording already active".to_string());
    }

    state.active = true;
    state.output_path = path.to_string();
    state.frame_count = 0;

    Ok(json!({ "started": true, "path": path }))
}

/// `record stop --expect-path <p>`: refuse unless the active recording writes
/// to exactly `p` (byte for byte, no path normalization).
pub fn check_expected_path(state: &RecordingState, expected: Option<&str>) -> Result<(), String> {
    let Some(expected) = expected else {
        return Ok(());
    };
    if !state.active {
        return Err("No recording in progress".to_string());
    }
    if state.output_path != expected {
        return Err(format!(
            "The active recording writes to {}, not {expected}; it was not stopped",
            state.output_path
        ));
    }
    Ok(())
}

pub fn recording_stop(state: &mut RecordingState) -> Result<Value, String> {
    if !state.active {
        return Err("No recording in progress".to_string());
    }

    state.active = false;

    if state.frame_count == 0 {
        return Err("No frames captured".to_string());
    }

    Ok(json!({ "path": &state.output_path, "frames": state.frame_count }))
}

pub fn recording_restart(state: &mut RecordingState, path: &str) -> Result<Value, String> {
    let previous = if state.active {
        let stop_result = recording_stop(state);
        stop_result
            .ok()
            .and_then(|v| v.get("path").and_then(|p| p.as_str()).map(String::from))
    } else {
        None
    };

    recording_start(state, path)?;

    Ok(json!({
        "restarted": true,
        "previousPath": previous,
        "path": path,
    }))
}

fn build_ffmpeg_command(output_path: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("ffmpeg");

    cmd.args(["-y"])
        .args(["-avioflags", "direct"])
        .args([
            "-fpsprobesize",
            "0",
            "-probesize",
            "32",
            "-analyzeduration",
            "0",
        ])
        .args([
            "-f",
            "image2pipe",
            "-c:v",
            "mjpeg",
            "-framerate",
            &CAPTURE_FPS.to_string(),
            "-i",
            "pipe:0",
        ])
        .args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2"]);

    if output_path.ends_with(".webm") {
        cmd.args(["-c:v", "libvpx", "-crf", "30", "-b:v", "1M"]);
    } else {
        cmd.args(["-c:v", "libx264", "-preset", "ultrafast"]);
    }

    cmd.args(["-pix_fmt", "yuv420p", "-threads", "1"])
        .arg(output_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    cmd
}

/// Spawn a background task that captures screenshots at a fixed interval
/// and pipes them to ffmpeg in real-time; a `.gif` path is encoded in process
/// instead.
pub fn spawn_recording_task(
    client: Arc<CdpClient>,
    session_id: String,
    output_path: String,
    shared_count: Arc<AtomicU64>,
    cancel_rx: oneshot::Receiver<()>,
) -> tokio::task::JoinHandle<Result<(), String>> {
    if is_gif_path(&output_path) {
        return spawn_gif_recording_task(client, session_id, output_path, shared_count, cancel_rx);
    }
    tokio::spawn(async move {
        let mut cancel_rx = std::pin::pin!(cancel_rx);

        let mut ffmpeg = build_ffmpeg_command(&output_path).spawn().map_err(|e| {
            format!(
                "ffmpeg not found or failed to execute: {}. Install ffmpeg to enable recording.",
                e
            )
        })?;

        let mut stdin = ffmpeg
            .stdin
            .take()
            .ok_or_else(|| "Failed to open ffmpeg stdin".to_string())?;

        let mut interval = tokio::time::interval(Duration::from_millis(CAPTURE_INTERVAL_MS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let params = CaptureScreenshotParams {
            format: Some("jpeg".to_string()),
            quality: Some(80),
            clip: None,
            from_surface: Some(true),
            capture_beyond_viewport: None,
        };

        loop {
            tokio::select! {
                _ = &mut cancel_rx => break,
                _ = interval.tick() => {}
            }

            let result: Result<CaptureScreenshotResult, _> = client
                .send_command_typed("Page.captureScreenshot", &params, Some(&session_id))
                .await;

            let screenshot = match result {
                Ok(s) => s,
                Err(e) => {
                    if e.contains("Target closed") || e.contains("not found") {
                        break;
                    }
                    continue;
                }
            };

            let bytes = match base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                &screenshot.data,
            ) {
                Ok(b) => b,
                Err(_) => continue,
            };

            if stdin.write_all(&bytes).await.is_err() {
                break;
            }
            shared_count.fetch_add(1, Ordering::Relaxed);
        }

        drop(stdin);

        let output = ffmpeg
            .wait_with_output()
            .await
            .map_err(|e| format!("ffmpeg wait failed: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "ffmpeg failed: {}",
                stderr.chars().take(300).collect::<String>()
            ));
        }

        Ok(())
    })
}

/// The GIF variant of [`spawn_recording_task`]: the same screenshots, every
/// [`GIF_INTERVAL_MS`], handed to a blocking thread that shrinks, quantizes
/// and appends each one to the file as it arrives, so memory stays flat
/// however long the recording runs.
fn spawn_gif_recording_task(
    client: Arc<CdpClient>,
    session_id: String,
    output_path: String,
    shared_count: Arc<AtomicU64>,
    cancel_rx: oneshot::Receiver<()>,
) -> tokio::task::JoinHandle<Result<(), String>> {
    tokio::spawn(async move {
        let mut cancel_rx = std::pin::pin!(cancel_rx);
        let file = std::fs::File::create(&output_path)
            .map_err(|e| format!("Cannot write {output_path}: {e}"))?;
        let (tx, rx) = std::sync::mpsc::sync_channel::<(Vec<u8>, Duration)>(GIF_QUEUE);
        let encoder = tokio::task::spawn_blocking(move || {
            let mut out = std::io::BufWriter::new(file);
            encode_gif_frames(&mut out, rx)?;
            std::io::Write::flush(&mut out).map_err(|e| format!("GIF write failed: {e}"))
        });

        let started = tokio::time::Instant::now();
        let mut interval = tokio::time::interval(Duration::from_millis(GIF_INTERVAL_MS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let params = CaptureScreenshotParams {
            format: Some("jpeg".to_string()),
            quality: Some(80),
            clip: None,
            from_surface: Some(true),
            capture_beyond_viewport: None,
        };

        loop {
            tokio::select! {
                _ = &mut cancel_rx => break,
                _ = interval.tick() => {}
            }
            let result: Result<CaptureScreenshotResult, _> = client
                .send_command_typed("Page.captureScreenshot", &params, Some(&session_id))
                .await;
            let screenshot = match result {
                Ok(s) => s,
                Err(e) => {
                    if e.contains("Target closed") || e.contains("not found") {
                        break;
                    }
                    continue;
                }
            };
            let Ok(bytes) = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                &screenshot.data,
            ) else {
                continue;
            };
            match tx.try_send((bytes, started.elapsed())) {
                Ok(()) => {
                    shared_count.fetch_add(1, Ordering::Relaxed);
                }
                Err(std::sync::mpsc::TrySendError::Full(_)) => continue,
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => break,
            }
        }

        drop(tx);
        encoder
            .await
            .map_err(|e| format!("GIF encoder failed: {e}"))?
    })
}

/// Encode screenshots (`(jpeg/png bytes, time since the recording started)`)
/// into a looping animated GIF on `out`. Each frame is shown until the next
/// one was taken; a frame identical to the one before only extends it. All
/// frames take the size of the first, scaled to at most [`GIF_MAX_WIDTH`].
/// Returns how many GIF frames were written.
pub(crate) fn encode_gif_frames<W: std::io::Write>(
    out: W,
    frames: impl IntoIterator<Item = (Vec<u8>, Duration)>,
) -> Result<u64, String> {
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::imageops::{self, FilterType};
    use image::{Delay, Frame, RgbaImage};

    let mut encoder = GifEncoder::new_with_speed(out, GIF_QUANT_SPEED);
    encoder
        .set_repeat(Repeat::Infinite)
        .map_err(|e| format!("GIF encoder: {e}"))?;
    let mut size: Option<(u32, u32)> = None;
    let mut pending: Option<(RgbaImage, Duration)> = None;
    let mut written = 0u64;
    let mut write = |encoder: &mut GifEncoder<W>, image: RgbaImage, shown: Duration| {
        // Browsers stretch delays under 20 ms to 100 ms.
        let shown = shown.max(Duration::from_millis(20));
        written += 1;
        encoder
            .encode_frame(Frame::from_parts(
                image,
                0,
                0,
                Delay::from_saturating_duration(shown),
            ))
            .map_err(|e| format!("GIF encoder: {e}"))
    };

    for (bytes, at) in frames {
        let Ok(decoded) = image::load_from_memory(&bytes) else {
            continue;
        };
        let (w, h) = *size.get_or_insert_with(|| {
            let (w, h) = (decoded.width().max(1), decoded.height().max(1));
            if w <= GIF_MAX_WIDTH {
                (w, h)
            } else {
                let scaled = (u64::from(h) * u64::from(GIF_MAX_WIDTH) / u64::from(w)).max(1);
                (GIF_MAX_WIDTH, scaled as u32)
            }
        });
        let mut frame = decoded.to_rgba8();
        if frame.dimensions() != (w, h) {
            frame = imageops::resize(&frame, w, h, FilterType::Triangle);
        }
        if let Some((previous, since)) = pending.take() {
            if previous.as_raw() == frame.as_raw() {
                pending = Some((previous, since));
                continue;
            }
            write(&mut encoder, previous, at.saturating_sub(since))?;
        }
        pending = Some((frame, at));
    }
    if let Some((last, _)) = pending {
        write(&mut encoder, last, Duration::from_millis(GIF_INTERVAL_MS))?;
    }
    drop(encoder);
    Ok(written)
}

pub async fn stop_recording_task(state: &mut RecordingState) -> Result<(), String> {
    if let Some(tx) = state.cancel_tx.take() {
        let _ = tx.send(());
    }

    let counter = state.shared_frame_count.take();
    let handle = state.capture_task.take();

    let result = if let Some(h) = handle {
        match h.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(e) => Err(format!("Recording task panicked: {}", e)),
        }
    } else {
        Ok(())
    };

    if let Some(c) = counter {
        state.frame_count = c.load(Ordering::Relaxed);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recording_state_new() {
        let state = RecordingState::new();
        assert!(!state.active);
        assert!(state.output_path.is_empty());
        assert_eq!(state.frame_count, 0);
    }

    #[test]
    fn test_recording_start_sets_active() {
        let mut state = RecordingState::new();
        let result = recording_start(&mut state, "/tmp/test.mp4");
        assert!(result.is_ok());
        assert!(state.active);
        assert_eq!(state.output_path, "/tmp/test.mp4");
        assert_eq!(state.frame_count, 0);
    }

    #[test]
    fn a_stop_for_another_recording_leaves_this_one_running() {
        let mut state = RecordingState::new();
        assert!(check_expected_path(&state, None).is_ok());
        assert!(check_expected_path(&state, Some("/w/a/recording.webm"))
            .unwrap_err()
            .contains("No recording in progress"));
        recording_start(&mut state, "/w/b/recording.webm").unwrap();
        let err = check_expected_path(&state, Some("/w/a/recording.webm")).unwrap_err();
        assert!(err.contains("not stopped"), "{err}");
        // Byte for byte: an equivalent spelling of the path is a different path.
        assert!(check_expected_path(&state, Some("/w/b/./recording.webm")).is_err());
        assert!(state.active);
        assert!(check_expected_path(&state, Some("/w/b/recording.webm")).is_ok());
        assert!(check_expected_path(&state, None).is_ok());
    }

    #[test]
    fn test_recording_start_while_active() {
        let mut state = RecordingState::new();
        recording_start(&mut state, "/tmp/test1.mp4").unwrap();
        let result = recording_start(&mut state, "/tmp/test2.mp4");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already active"));
    }

    #[test]
    fn test_recording_stop_not_active() {
        let mut state = RecordingState::new();
        let result = recording_stop(&mut state);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No recording"));
    }

    #[test]
    fn test_recording_stop_no_frames() {
        let mut state = RecordingState::new();
        recording_start(&mut state, "/tmp/test.mp4").unwrap();
        let result = recording_stop(&mut state);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No frames"));
        assert!(!state.active);
    }

    #[test]
    fn test_recording_restart_while_inactive() {
        let mut state = RecordingState::new();
        let result = recording_restart(&mut state, "/tmp/new.webm");
        assert!(result.is_ok());
        assert!(state.active);
        assert_eq!(state.output_path, "/tmp/new.webm");
    }

    #[test]
    fn test_recording_restart_while_active() {
        let mut state = RecordingState::new();
        recording_start(&mut state, "/tmp/old.webm").unwrap();
        state.frame_count = 10;
        let result = recording_restart(&mut state, "/tmp/new.webm").unwrap();
        assert!(state.active);
        assert_eq!(state.output_path, "/tmp/new.webm");
        assert_eq!(state.frame_count, 0);
        assert_eq!(result["previousPath"], "/tmp/old.webm");
    }

    #[test]
    fn gif_paths_are_recognised_by_extension() {
        assert!(is_gif_path("/tmp/demo.gif"));
        assert!(is_gif_path("demo.GIF"));
        assert!(!is_gif_path("/tmp/demo.webm"));
        assert!(!is_gif_path("/tmp/gif"));
        assert!(!is_gif_path("/tmp/demo.gif.webm"));
    }

    fn jpeg(width: u32, height: u32, shade: u8) -> Vec<u8> {
        let image = image::RgbImage::from_pixel(width, height, image::Rgb([shade, 40, 200]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut out, image::ImageFormat::Jpeg)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn a_gif_is_animated_shrunk_to_800_wide_and_merges_repeats() {
        use image::AnimationDecoder;

        let ms = Duration::from_millis;
        let frames = vec![
            (jpeg(1280, 720, 10), ms(0)),
            (jpeg(1280, 720, 10), ms(200)), // unchanged: extends the first
            (jpeg(1280, 720, 250), ms(400)),
            (b"not an image".to_vec(), ms(500)), // skipped
            (jpeg(1600, 900, 120), ms(600)),     // a resized window
        ];
        let mut gif = Vec::new();
        let written = encode_gif_frames(&mut gif, frames).unwrap();
        assert_eq!(written, 3);
        assert!(gif.starts_with(b"GIF89a"));

        let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(&gif)).unwrap();
        let decoded = decoder.into_frames().collect_frames().unwrap();
        assert_eq!(decoded.len(), 3);
        for frame in &decoded {
            assert_eq!(frame.buffer().dimensions(), (800, 450));
        }
        let delays: Vec<u32> = decoded
            .iter()
            .map(|f| {
                let (n, d) = f.delay().numer_denom_ms();
                n / d
            })
            .collect();
        assert_eq!(delays, vec![400, 200, 200]);
    }

    #[test]
    fn a_gif_with_no_decodable_frame_writes_nothing() {
        let mut gif = Vec::new();
        let written = encode_gif_frames(&mut gif, vec![(b"x".to_vec(), Duration::ZERO)]).unwrap();
        assert_eq!(written, 0);
        assert!(gif.is_empty());
    }

    #[test]
    fn test_build_ffmpeg_command_webm() {
        let cmd = build_ffmpeg_command("/tmp/out.webm");
        let args: Vec<&std::ffi::OsStr> = cmd.as_std().get_args().collect();
        let args_str: Vec<&str> = args.iter().filter_map(|a| a.to_str()).collect();
        assert!(args_str.contains(&"libvpx"));
        assert!(args_str.contains(&"/tmp/out.webm"));
    }

    #[test]
    fn test_build_ffmpeg_command_mp4() {
        let cmd = build_ffmpeg_command("/tmp/out.mp4");
        let args: Vec<&std::ffi::OsStr> = cmd.as_std().get_args().collect();
        let args_str: Vec<&str> = args.iter().filter_map(|a| a.to_str()).collect();
        assert!(args_str.contains(&"libx264"));
        assert!(args_str.contains(&"/tmp/out.mp4"));
    }
}

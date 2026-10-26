//! Opt-in local frame extraction for native site adapters. No browser or network
//! request is made until the requested frame and output file have been checked.
use serde_json::{json, Value};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn prepare(meta: &Value, args: &mut Value) -> Result<(), String> {
    // Custom adapters retain their own frame semantics unless they opt in.
    if meta.get("coverFrame").is_none() {
        return Ok(());
    }
    let mode = args["cover-mode"].as_str().unwrap_or("").to_string();
    let frame = mode.strip_prefix("frame:");
    let local_flags = ["cover-video", "cover-output", "allow-dark-cover"];
    if frame.is_none() {
        if meta.get("coverFrame").is_some() && local_flags.iter().any(|k| args.get(k).is_some()) {
            return Err("site: --cover-video/--cover-output/--allow-dark-cover require --cover-mode frame:<seconds>; no adapter was invoked".into());
        }
        return Ok(());
    }
    let settings = meta.get("coverFrame").ok_or(
        "site: this adapter does not opt in to local frame covers; no adapter was invoked",
    )?;
    let targets: Vec<&str> = settings["targets"]
        .as_array()
        .ok_or("site: invalid coverFrame targets")?
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()
        .ok_or("site: every coverFrame target must be a file argument name")?;
    if targets.is_empty() || targets.iter().any(|k| meta["args"][k]["type"] != "file") {
        return Err("site: coverFrame targets must name declared file arguments".into());
    }
    if targets.iter().any(|k| args.get(k).is_some()) {
        return Err("site: frame mode cannot be combined with explicit cover files".into());
    }
    let seconds = parse_seconds(frame.unwrap())?;
    let source = args["cover-video"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("site: frame mode requires --cover-video <local-video>")?;
    let output = args["cover-output"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or(
            "site: frame mode requires --cover-output <new.png>; the image is retained for review",
        )?;
    let allow_dark = match args.get("allow-dark-cover").and_then(Value::as_str) {
        None | Some("false") => false,
        Some("true") => true,
        _ => return Err("site: --allow-dark-cover expects true or false".into()),
    };
    let frame = extract(Path::new(source), Path::new(output), seconds)?;
    // Keep even a refused frame as the explicitly requested review artifact.
    if frame.suspicious && !allow_dark {
        return Err(format!("cover_frame_review_required: requested frame is near-black or nearly uniform; image saved to {}. Choose another timestamp, or review it and explicitly pass --allow-dark-cover true with a new output path. No adapter was invoked", frame.path));
    }
    let obj = args
        .as_object_mut()
        .ok_or("site: expected argument object")?;
    for key in local_flags {
        obj.remove(key);
    }
    for target in targets {
        obj.insert(target.into(), Value::String(frame.path.clone()));
    }
    obj.insert("cover-mode".into(), json!("upload"));
    obj.insert("frameCover".into(), json!({"seconds":seconds,"output":frame.path,"width":frame.width,"height":frame.height,"nearBlackOrUniform":frame.suspicious,"explicitlyAccepted":allow_dark}));
    Ok(())
}

fn parse_seconds(raw: &str) -> Result<f64, String> {
    let seconds = raw
        .parse::<f64>()
        .map_err(|_| "site: frame timestamp must be finite, nonnegative seconds")?;
    if !seconds.is_finite() || !(0.0..=604800.0).contains(&seconds) {
        return Err("site: frame timestamp must be finite and between 0 and 604800 seconds".into());
    }
    Ok(seconds)
}

struct Frame {
    path: String,
    width: u32,
    height: u32,
    suspicious: bool,
}
fn extract(source: &Path, output: &Path, seconds: f64) -> Result<Frame, String> {
    let source = source
        .canonicalize()
        .map_err(|e| format!("site: cover video: {e}"))?;
    if !source.is_file() {
        return Err("site: --cover-video must be a regular local file".into());
    }
    if output
        .extension()
        .and_then(|s| s.to_str())
        .is_none_or(|s| !s.eq_ignore_ascii_case("png"))
    {
        return Err("site: --cover-output must name a new .png file".into());
    }
    if output.exists() {
        return Err(
            "site: cover output already exists; choose a new path (nothing was overwritten)".into(),
        );
    }
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = parent
        .canonicalize()
        .map_err(|e| format!("site: cover output directory: {e}"))?;
    let output = parent.join(
        output
            .file_name()
            .ok_or("site: missing cover output filename")?,
    );
    let temporary = tempfile::Builder::new()
        .prefix(".chrome-use-cover-")
        .suffix(".png")
        .tempfile_in(&parent)
        .map_err(|e| e.to_string())?;
    let mut process = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-protocol_whitelist",
            "file,pipe",
            "-ss",
            &seconds.to_string(),
            "-i",
        ])
        .arg(&source)
        .args([
            "-map",
            "0:v:0",
            "-frames:v",
            "1",
            "-vf",
            "scale=w='min(1920,iw)':h='min(1080,ih)':force_original_aspect_ratio=decrease",
            "-f",
            "image2",
        ])
        .arg(temporary.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("site: ffmpeg is required for frame covers: {e}"))?;
    let start = Instant::now();
    let status = loop {
        match process.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < Duration::from_secs(30) => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = process.kill();
                let _ = process.wait();
                return Err(
                    "site: frame extraction failed or exceeded 30 seconds; no adapter was invoked"
                        .into(),
                );
            }
        }
    };
    if !status.success() {
        return Err(
            "site: ffmpeg could not extract the requested video frame; no adapter was invoked"
                .into(),
        );
    }
    let bytes = std::fs::read(temporary.path()).map_err(|e| e.to_string())?;
    if bytes.is_empty() || bytes.len() > 16 * 1024 * 1024 {
        return Err(
            "site: no bounded image at the requested timestamp (possibly beyond video duration)"
                .into(),
        );
    }
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .map_err(|e| format!("site: extracted frame is not a valid PNG: {e}"))?;
    if image.width() == 0 || image.height() == 0 || image.width() > 1920 || image.height() > 1080 {
        return Err("site: extracted image dimensions are invalid".into());
    }
    let suspicious = suspicious_image(&image);
    temporary
        .persist_noclobber(&output)
        .map_err(|e| format!("site: cannot save cover output without overwriting: {e}"))?;
    Ok(Frame {
        path: output.to_string_lossy().into(),
        width: image.width(),
        height: image.height(),
        suspicious,
    })
}
fn suspicious_image(image: &image::DynamicImage) -> bool {
    let sample = image.thumbnail(64, 64).to_rgb8();
    let mut sum = [0.0; 3];
    let mut squares = [0.0; 3];
    let mut dark = 0usize;
    let n = sample.len() as f64 / 3.0;
    for p in sample.pixels() {
        if p.0.iter().all(|v| *v < 16) {
            dark += 1;
        }
        for i in 0..3 {
            let v = f64::from(p[i]);
            sum[i] += v;
            squares[i] += v * v;
        }
    }
    let uniform = (0..3).all(|i| (squares[i] / n - (sum[i] / n).powi(2)).max(0.0).sqrt() < 3.0);
    uniform || dark as f64 / n > 0.98
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamps_and_image_review_are_explicit() {
        for t in ["-1", "NaN", "inf", "", "1;rm", "604801"] {
            assert!(parse_seconds(t).is_err(), "{t}");
        }
        assert_eq!(parse_seconds("1.25").unwrap(), 1.25);
        for color in [[0, 0, 0], [255, 255, 255], [30, 120, 60]] {
            assert!(suspicious_image(&image::DynamicImage::ImageRgb8(
                image::RgbImage::from_pixel(64, 64, image::Rgb(color))
            )));
        }
        let varied = image::RgbImage::from_fn(64, 64, |x, y| {
            image::Rgb([(x * 4) as u8, (y * 4) as u8, 120])
        });
        assert!(!suspicious_image(&image::DynamicImage::ImageRgb8(varied)));
    }
    #[test]
    fn no_ignored_frame_flags_or_custom_adapter_changes() {
        let meta = json!({"coverFrame":{"targets":["cover"]},"args":{"cover":{"type":"file"}}});
        assert!(prepare(&meta, &mut json!({"cover-video":"x"})).is_err());
        assert!(prepare(&meta, &mut json!({"cover-mode":"frame:2","cover":"a.png"})).is_err());
        assert!(prepare(&json!({}), &mut json!({"cover-mode":"frame:2"})).is_ok());
        assert!(prepare(&json!({}), &mut json!({"custom":"keep"})).is_ok());
    }
    #[test]
    #[ignore = "requires ffmpeg; run on the configured build host"]
    fn exact_frame_extraction_is_bounded_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("source.mkv");
        let ok = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=red:s=64x48:r=10:d=1",
                "-f",
                "lavfi",
                "-i",
                "color=blue:s=64x48:r=10:d=1",
                "-filter_complex",
                "[0:v][1:v]concat=n=2:v=1:a=0",
                "-c:v",
                "ffv1",
            ])
            .arg(&video)
            .status()
            .unwrap();
        assert!(ok.success());
        let output = dir.path().join("frame.png");
        let frame = extract(&video, &output, 1.5).unwrap();
        assert_eq!((frame.width, frame.height), (64, 48));
        let rgb = image::open(&output).unwrap().to_rgb8();
        let p = rgb.get_pixel(20, 20);
        assert!(p[2] > 200 && p[0] < 30, "{p:?}");
        assert!(frame.suspicious);
        assert!(extract(&video, &output, 0.2).is_err());
        assert_eq!(
            *rgb.get_pixel(20, 20),
            *image::open(&output).unwrap().to_rgb8().get_pixel(20, 20)
        );
        assert!(extract(&video, &dir.path().join("beyond.png"), 5.0).is_err());
        assert!(!dir.path().join("beyond.png").exists());
        let meta = json!({"coverFrame":{"targets":["cover"]},"args":{"cover":{"type":"file"}}});
        let refused = dir.path().join("refused.png");
        let mut args = json!({"cover-mode":"frame:1.5","cover-video":video,"cover-output":refused});
        assert!(prepare(&meta, &mut args)
            .unwrap_err()
            .contains("cover_frame_review_required"));
        assert!(refused.exists());
        let accepted = dir.path().join("accepted.png");
        let mut args = json!({"cover-mode":"frame:1.5","cover-video":video,"cover-output":accepted,"allow-dark-cover":"true"});
        prepare(&meta, &mut args).unwrap();
        assert_eq!(args["cover-mode"], "upload");
        assert!(args.get("cover-video").is_none());
        assert_eq!(args["frameCover"]["seconds"], 1.5);
        let adapter = crate::site::Adapter {
            meta: json!({"coverFrame":{"targets":["cover-portrait","cover-landscape"]}, "args":{"cover-portrait":{"type":"file"},"cover-landscape":{"type":"file"},"cover-video":{"type":"file"}}}),
            func_src: "async function(args){return args}".into(),
            arg_order: vec![],
            spec: "fixture/cover".into(),
        };
        let inv = crate::site::prepare_invocation(
            &adapter,
            &[],
            &[
                ("cover-mode".into(), "frame:1.5".into()),
                ("cover-video".into(), video.display().to_string()),
                (
                    "cover-output".into(),
                    dir.path().join("invocation.png").display().to_string(),
                ),
                ("allow-dark-cover".into(), "true".into()),
            ],
            false,
            &mut || Err("unexpected stdin".into()),
        )
        .unwrap();
        assert_eq!(
            inv.files["cover-portrait"]["path"],
            inv.files["cover-landscape"]["path"]
        );
        assert!(inv.files.get("cover-video").is_none());
        assert_eq!(inv.args["cover-mode"], "upload");
        assert!(Path::new(inv.files["cover-portrait"]["path"].as_str().unwrap()).is_file());
    }
}

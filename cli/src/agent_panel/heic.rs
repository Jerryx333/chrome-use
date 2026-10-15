//! Convert HEIC/HEIF photos to JPEG with the platform's own codec. Chrome
//! cannot decode HEIC, so the panel sends the original file here and then
//! normalizes the JPEG like any other attachment. The reply must stay well
//! under the 1 MiB host-to-extension native-messaging limit.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{
    io::Cursor,
    path::Path,
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

const MAX_SOURCE_BYTES: usize = 20 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 600 * 1024;
const MAX_EDGE: u32 = 2000;
const TIMEOUT: Duration = Duration::from_secs(20);
const BRANDS: [&[u8; 4]; 8] = [
    b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"mif1", b"msf1",
];

pub(super) fn handle(params: &Value) -> Result<Value, String> {
    let request = params
        .as_object()
        .filter(|p| p.len() == 1)
        .and_then(|p| p.get("data"))
        .and_then(Value::as_str)
        .ok_or("imageConvert expects only base64 image data")?;
    if request.len() > MAX_SOURCE_BYTES.div_ceil(3) * 4 {
        return Err("The source image exceeds 20 MiB".into());
    }
    let source = STANDARD
        .decode(request)
        .map_err(|_| "Invalid image encoding")?;
    if source.is_empty() || source.len() > MAX_SOURCE_BYTES {
        return Err("The source image exceeds 20 MiB".into());
    }
    if !is_heif(&source) {
        return Err("This file is not a HEIC or HEIF image".into());
    }
    let jpeg = convert_with(&source, platform_convert)?;
    Ok(json!({"schemaVersion":1,"mediaType":"image/jpeg","data":STANDARD.encode(jpeg)}))
}

/// The ISO-BMFF `ftyp` box names HEIF in its major or a compatible brand.
fn is_heif(bytes: &[u8]) -> bool {
    if bytes.len() < 16 || &bytes[4..8] != b"ftyp" {
        return false;
    }
    let size = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if !(16..=4096).contains(&size) || size > bytes.len() {
        return false;
    }
    let (compatible, _) = bytes[16..size].as_chunks::<4>();
    std::iter::once(&[bytes[8], bytes[9], bytes[10], bytes[11]])
        .chain(compatible)
        .any(|brand| BRANDS.contains(&brand))
}

/// The temporary directory is private to the user and removed on every path.
fn convert_with(
    source: &[u8],
    converter: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> Result<Vec<u8>, String> {
    let directory = tempfile::Builder::new()
        .prefix("chrome-use-heic-")
        .tempdir()
        .map_err(|_| "Cannot create a private temporary folder")?;
    let input = directory.path().join("source.heic");
    let output = directory.path().join("converted.jpg");
    std::fs::write(&input, source).map_err(|_| "Cannot stage the image for conversion")?;
    converter(&input, &output)?;
    let length = std::fs::metadata(&output)
        .map_err(|_| "The image converter produced no output")?
        .len();
    if length == 0 || length > 64 * 1024 * 1024 {
        return Err("The image converter produced an unusable file".into());
    }
    let jpeg = std::fs::read(&output).map_err(|_| "Cannot read the converted image")?;
    fit(jpeg)
}

/// Accept only a decodable JPEG and always re-encode it: the EXIF orientation
/// is baked into the pixels, and the new file carries no EXIF at all, so GPS
/// and camera metadata never reach the agent.
fn fit(jpeg: Vec<u8>) -> Result<Vec<u8>, String> {
    use image::ImageDecoder;
    if image::guess_format(&jpeg).ok() != Some(image::ImageFormat::Jpeg) {
        return Err("The image converter did not produce a JPEG".into());
    }
    let mut decoder = image::codecs::jpeg::JpegDecoder::new(Cursor::new(&jpeg))
        .map_err(|_| "The converted image is invalid")?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(512 * 1024 * 1024);
    decoder
        .set_limits(limits)
        .map_err(|_| "The converted image is too large")?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut picture =
        image::DynamicImage::from_decoder(decoder).map_err(|_| "The converted image is invalid")?;
    picture.apply_orientation(orientation);
    if picture.width().max(picture.height()) > MAX_EDGE {
        picture = picture.resize(MAX_EDGE, MAX_EDGE, image::imageops::FilterType::Triangle);
    }
    let picture = picture.to_rgb8();
    for quality in [90, 80, 65, 50, 40] {
        let mut encoded = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, quality)
            .encode_image(&picture)
            .map_err(|_| "Cannot encode the converted image")?;
        if encoded.len() <= MAX_OUTPUT_BYTES {
            return Ok(encoded);
        }
    }
    Err("This image is still too large; crop or resize it before attaching".into())
}

/// Run a converter without a shell, killing it once the time budget is spent.
fn run(mut command: Command, timeout: Duration) -> Result<ExitStatus, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Cannot start the image converter")?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Converting the HEIC image timed out".into());
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn platform_convert(input: &Path, output: &Path) -> Result<(), String> {
    let mut command = Command::new("/usr/bin/sips");
    command
        .args(["-s", "format", "jpeg", "-s", "formatOptions", "90"])
        .arg(input)
        .arg("--out")
        .arg(output);
    if run(command, TIMEOUT)?.success() {
        Ok(())
    } else {
        Err("macOS could not convert this HEIC image; export it as JPEG and attach that".into())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_convert(input: &Path, output: &Path) -> Result<(), String> {
    let command = if let Some(tool) = find_tool("heif-convert") {
        let mut command = Command::new(tool);
        command.args(["-q", "90"]).arg(input).arg(output);
        command
    } else if let Some(tool) = find_tool("magick") {
        let mut command = Command::new(tool);
        command
            .arg(input)
            .args(["-auto-orient", "-resize", "2000x2000>", "-quality", "90"])
            .arg(output);
        command
    } else {
        return Err("Converting HEIC needs heif-convert (libheif) or ImageMagick with HEIF support; install one, or export the photo as JPEG".into());
    };
    if run(command, TIMEOUT)?.success() {
        Ok(())
    } else {
        Err("The installed HEIC converter could not read this image; export it as JPEG and attach that".into())
    }
}

/// Chrome may start the native host with a minimal PATH.
#[cfg(all(unix, not(target_os = "macos")))]
fn find_tool(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/usr/bin", "/usr/local/bin"].map(std::path::PathBuf::from))
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

/// WPF's imaging stack uses the WIC HEIF codec from the Microsoft Store.
#[cfg(windows)]
const WINDOWS_SCRIPT: &str = r#"$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName PresentationCore
$source = [IO.File]::OpenRead($env:CHROME_USE_HEIC_IN)
try {
  try { $decoder = [Windows.Media.Imaging.BitmapDecoder]::Create($source, [Windows.Media.Imaging.BitmapCreateOptions]::None, [Windows.Media.Imaging.BitmapCacheOption]::OnLoad) } catch { exit 3 }
  $frame = $decoder.Frames[0]
  $orientation = 1
  foreach ($query in 'System.Photo.Orientation', '/app1/ifd/{ushort=274}', '/ifd/{ushort=274}') {
    try { $value = $frame.Metadata.GetQuery($query); if ($value -ge 1 -and $value -le 8) { $orientation = [int]$value; break } } catch { }
  }
  # EXIF orientation as a horizontal flip followed by a clockwise rotation.
  $flip = @{2=$true; 4=$true; 5=$true; 7=$true}[$orientation] -eq $true
  $angle = @{3=180; 4=180; 5=270; 6=90; 7=90; 8=270}[$orientation]; if ($null -eq $angle) { $angle = 0 }
  $scale = [Math]::Min(1.0, 2000.0 / [Math]::Max($frame.PixelWidth, $frame.PixelHeight))
  $transform = New-Object Windows.Media.TransformGroup
  $transform.Children.Add((New-Object Windows.Media.ScaleTransform($(if ($flip) { -$scale } else { $scale }), $scale)))
  $transform.Children.Add((New-Object Windows.Media.RotateTransform($angle)))
  $bitmap = New-Object Windows.Media.Imaging.TransformedBitmap($frame, $transform)
  $encoder = New-Object Windows.Media.Imaging.JpegBitmapEncoder
  $encoder.QualityLevel = 90
  $encoder.Frames.Add([Windows.Media.Imaging.BitmapFrame]::Create($bitmap))
  $target = [IO.File]::Create($env:CHROME_USE_HEIC_OUT)
  try { $encoder.Save($target) } finally { $target.Close() }
} finally { $source.Close() }"#;

#[cfg(windows)]
fn platform_convert(input: &Path, output: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let mut command =
        Command::new(Path::new(&root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe"));
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            WINDOWS_SCRIPT,
        ])
        .env("CHROME_USE_HEIC_IN", input)
        .env("CHROME_USE_HEIC_OUT", output)
        .creation_flags(CREATE_NO_WINDOW);
    let status = run(command, TIMEOUT)?;
    windows_result(status.code())
}

#[cfg_attr(not(windows), allow(dead_code))]
fn windows_result(code: Option<i32>) -> Result<(), String> {
    match code {
        Some(0) => Ok(()),
        Some(3) => Err("HEIC needs the HEIF Image Extensions and HEVC Video Extensions from the Microsoft Store; install them, or export the photo as JPEG".into()),
        _ => Err("Windows could not convert this HEIC image; export it as JPEG and attach that".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ftyp(major: &[u8; 4], compatible: &[&[u8; 4]]) -> Vec<u8> {
        let size = 16 + 4 * compatible.len() as u32;
        let mut bytes = size.to_be_bytes().to_vec();
        bytes.extend_from_slice(b"ftyp");
        bytes.extend_from_slice(major);
        bytes.extend_from_slice(&[0; 4]);
        for brand in compatible {
            bytes.extend_from_slice(*brand);
        }
        bytes.extend_from_slice(&[0; 32]);
        bytes
    }

    fn jpeg(width: u32, height: u32, noisy: bool) -> Vec<u8> {
        let picture = image::RgbImage::from_fn(width, height, |x, y| {
            if noisy {
                let v = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) as u8;
                image::Rgb([v, v.wrapping_mul(7), v.wrapping_mul(13)])
            } else {
                image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
            }
        });
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 95)
            .encode_image(&picture)
            .unwrap();
        bytes
    }

    /// A landscape picture whose left sixth is red, so rotation is observable.
    fn striped(width: u32, height: u32) -> Vec<u8> {
        let picture = image::RgbImage::from_fn(width, height, |x, _| {
            if x < width / 6 {
                image::Rgb([255, 0, 0])
            } else {
                image::Rgb([0, 0, 255])
            }
        });
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 95)
            .encode_image(&picture)
            .unwrap();
        bytes
    }

    /// Insert an EXIF APP1 segment with an orientation and a GPS position.
    fn with_exif(jpeg: &[u8], orientation: u16) -> Vec<u8> {
        let mut tiff = b"MM\0\x2a\0\0\0\x08".to_vec();
        tiff.extend_from_slice(&2u16.to_be_bytes());
        tiff.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1]);
        tiff.extend_from_slice(&orientation.to_be_bytes());
        tiff.extend_from_slice(&[0, 0]);
        tiff.extend_from_slice(&[0x88, 0x25, 0, 4, 0, 0, 0, 1, 0, 0, 0, 38]);
        tiff.extend_from_slice(&[0; 4]);
        tiff.extend_from_slice(&2u16.to_be_bytes());
        tiff.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 4, 2, 2, 0, 0]);
        tiff.extend_from_slice(&[0, 1, 0, 2, 0, 0, 0, 2, b'N', 0, 0, 0]);
        tiff.extend_from_slice(&[0; 4]);
        let mut segment = vec![0xff, 0xe1];
        segment.extend_from_slice(&(tiff.len() as u16 + 8).to_be_bytes());
        segment.extend_from_slice(b"Exif\0\0");
        segment.extend_from_slice(&tiff);
        [&jpeg[..2], &segment, &jpeg[2..]].concat()
    }

    fn has_exif(jpeg: &[u8]) -> bool {
        jpeg.windows(6).any(|w| w == b"Exif\0\0")
    }

    fn assert_upright_portrait(jpeg: &[u8], width: u32, height: u32) {
        assert!(!has_exif(jpeg), "EXIF (orientation, GPS) must be stripped");
        let picture = image::load_from_memory(jpeg).unwrap().to_rgb8();
        assert_eq!(picture.dimensions(), (width, height));
        let top = picture.get_pixel(width / 2, 2);
        let bottom = picture.get_pixel(width / 2, height - 3);
        assert!(top[0] > 200 && top[2] < 60, "top is red: {top:?}");
        assert!(
            bottom[2] > 200 && bottom[0] < 60,
            "bottom is blue: {bottom:?}"
        );
    }

    #[test]
    fn brand_sniff_accepts_heif_brands_and_rejects_others() {
        assert!(is_heif(&ftyp(b"heic", &[b"mif1"])));
        assert!(is_heif(&ftyp(b"mif1", &[])));
        assert!(is_heif(&ftyp(b"isom", &[b"mp41", b"heix"])));
        assert!(!is_heif(&ftyp(b"mp41", &[b"isom", b"avif"])));
        assert!(!is_heif(&jpeg(2, 2, false)));
        assert!(!is_heif(b"\0\0\0\x18ftypheic"));
        let mut lying = ftyp(b"heic", &[]);
        lying[..4].copy_from_slice(&100_000u32.to_be_bytes());
        assert!(!is_heif(&lying));
        let mut short = ftyp(b"isom", &[b"heic"]);
        short[..4].copy_from_slice(&16u32.to_be_bytes());
        assert!(!is_heif(&short), "brands past the box end are ignored");
    }

    #[test]
    fn requests_are_bounded_and_checked_before_conversion() {
        let oversized = "A".repeat(MAX_SOURCE_BYTES.div_ceil(3) * 4 + 4);
        assert!(handle(&json!({"data": oversized}))
            .unwrap_err()
            .contains("20 MiB"));
        assert!(handle(&json!({"data":"!!"}))
            .unwrap_err()
            .contains("encoding"));
        assert!(handle(&json!({"data": STANDARD.encode(jpeg(4, 4, false))}))
            .unwrap_err()
            .contains("not a HEIC"));
        assert!(handle(&json!({"data":"", "name":"x.heic"})).is_err());
        assert!(handle(&json!([])).is_err());
    }

    #[test]
    fn output_is_reduced_and_never_upscaled() {
        let small = image::load_from_memory(&fit(jpeg(40, 30, false)).unwrap()).unwrap();
        assert_eq!((small.width(), small.height()), (40, 30));
        let reduced = fit(jpeg(3000, 1500, true)).unwrap();
        assert!(reduced.len() <= MAX_OUTPUT_BYTES, "{}", reduced.len());
        let picture = image::load_from_memory(&reduced).unwrap();
        assert_eq!((picture.width(), picture.height()), (2000, 1000));
        assert!(fit(b"not a jpeg".to_vec()).unwrap_err().contains("JPEG"));
    }

    #[test]
    fn orientation_is_baked_into_pixels_and_metadata_stripped() {
        let tagged = with_exif(&striped(300, 120), 6);
        assert!(has_exif(&tagged));
        assert_upright_portrait(&fit(tagged).unwrap(), 120, 300);
        let plain = fit(with_exif(&striped(300, 120), 1)).unwrap();
        assert!(!has_exif(&plain));
        assert_eq!(image::load_from_memory(&plain).unwrap().width(), 300);
    }

    #[test]
    fn temporary_files_are_removed_after_success_and_failure() {
        let mut seen = None;
        let out = convert_with(b"source", |input, output| {
            seen = Some(input.parent().unwrap().to_path_buf());
            std::fs::write(output, jpeg(8, 8, false)).unwrap();
            Ok(())
        })
        .unwrap();
        assert!(image::load_from_memory(&out).is_ok());
        assert!(!seen.take().unwrap().exists());
        let error = convert_with(b"source", |input, _| {
            seen = Some(input.parent().unwrap().to_path_buf());
            Err("codec missing".into())
        })
        .unwrap_err();
        assert_eq!(error, "codec missing");
        assert!(!seen.unwrap().exists());
        assert!(convert_with(b"source", |_, _| Ok(()))
            .unwrap_err()
            .contains("no output"));
    }

    #[test]
    fn windows_codec_failure_names_the_store_extensions() {
        assert!(windows_result(Some(0)).is_ok());
        assert!(windows_result(Some(3))
            .unwrap_err()
            .contains("HEIF Image Extensions"));
        assert!(windows_result(None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn slow_converters_are_killed_at_the_deadline() {
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        let started = Instant::now();
        assert!(run(command, Duration::from_millis(200))
            .unwrap_err()
            .contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// Round-trips a real HEIC written by the system codec.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_converts_a_real_heic() {
        let directory = tempfile::tempdir().unwrap();
        let png = directory.path().join("source.png");
        let heic = directory.path().join("source.heic");
        image::RgbImage::from_fn(2400, 1200, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        })
        .save(&png)
        .unwrap();
        let status = Command::new("/usr/bin/sips")
            .args(["-s", "format", "heic"])
            .arg(&png)
            .arg("--out")
            .arg(&heic)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "sips could not write HEIC");
        let source = std::fs::read(&heic).unwrap();
        assert!(is_heif(&source));
        let reply = handle(&json!({"data": STANDARD.encode(source)})).unwrap();
        assert_eq!(reply["mediaType"], "image/jpeg");
        let bytes = STANDARD.decode(reply["data"].as_str().unwrap()).unwrap();
        assert!(bytes.len() <= MAX_OUTPUT_BYTES);
        let picture = image::load_from_memory(&bytes).unwrap();
        assert_eq!((picture.width(), picture.height()), (2000, 1000));
    }

    /// iPhone portraits store landscape pixels plus a rotation; sips keeps
    /// the stored pixels and copies the EXIF orientation (and GPS) into its
    /// JPEG, so the module must rotate and strip.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_rotated_heic_comes_out_upright_without_exif() {
        let directory = tempfile::tempdir().unwrap();
        let tagged = directory.path().join("tagged.jpg");
        let heic = directory.path().join("rotated.heic");
        std::fs::write(&tagged, with_exif(&striped(300, 120), 6)).unwrap();
        let status = Command::new("/usr/bin/sips")
            .args(["-s", "format", "heic"])
            .arg(&tagged)
            .arg("--out")
            .arg(&heic)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "sips could not write HEIC");
        let source = std::fs::read(&heic).unwrap();
        assert!(source.windows(4).any(|w| w == b"irot"), "HEIC carries irot");
        let reply = handle(&json!({"data": STANDARD.encode(source)})).unwrap();
        let bytes = STANDARD.decode(reply["data"].as_str().unwrap()).unwrap();
        assert_upright_portrait(&bytes, 120, 300);
    }
}

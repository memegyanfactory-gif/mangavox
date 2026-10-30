//! Export encoders: detects which ffmpeg encoders really work on this machine and builds the
//! output arguments for the export presets, preferring the GPU encoder and falling back to
//! libx264 / libx265. Ported from bhippi-Video-editor (MIT) `exportPresets.ts` / `codec.rs`.
//! Self-contained; the export dialog wires it in later.
#![allow(dead_code)]
use std::process::Stdio;
use std::sync::OnceLock;

use mv_studio::python::quiet_command;

/// Video encoders we know how to drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoder {
    H264Nvenc,
    HevcNvenc,
    H264Qsv,
    H264Amf,
    Libx264,
    Libx265,
}

impl Encoder {
    pub const ALL: [Encoder; 6] = [Encoder::H264Nvenc, Encoder::HevcNvenc, Encoder::H264Qsv, Encoder::H264Amf, Encoder::Libx264, Encoder::Libx265];

    /// The ffmpeg `-c:v` name.
    pub fn name(self) -> &'static str {
        match self {
            Encoder::H264Nvenc => "h264_nvenc",
            Encoder::HevcNvenc => "hevc_nvenc",
            Encoder::H264Qsv => "h264_qsv",
            Encoder::H264Amf => "h264_amf",
            Encoder::Libx264 => "libx264",
            Encoder::Libx265 => "libx265",
        }
    }

    pub fn is_gpu(self) -> bool {
        !matches!(self, Encoder::Libx264 | Encoder::Libx265)
    }

    pub fn is_hevc(self) -> bool {
        matches!(self, Encoder::HevcNvenc | Encoder::Libx265)
    }
}

/// The encoders usable here.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Encoders {
    pub available: Vec<Encoder>,
}

impl Encoders {
    pub fn has(&self, e: Encoder) -> bool {
        self.available.contains(&e)
    }

    /// Software only (what to assume when ffmpeg can't be asked).
    pub fn software() -> Self {
        Self { available: vec![Encoder::Libx264, Encoder::Libx265] }
    }

    /// Best encoder for the codec family: GPU first, then software; libx264 as the last resort.
    pub fn pick(&self, hevc: bool) -> Encoder {
        let order: &[Encoder] = if hevc { &[Encoder::HevcNvenc, Encoder::Libx265] } else { &[Encoder::H264Nvenc, Encoder::H264Qsv, Encoder::H264Amf, Encoder::Libx264] };
        order.iter().copied().find(|e| self.has(*e)).unwrap_or(Encoder::Libx264)
    }

    pub fn has_hevc(&self) -> bool {
        self.has(Encoder::HevcNvenc) || self.has(Encoder::Libx265)
    }
}

/// Reads the encoder names we care about out of `ffmpeg -hide_banner -encoders`.
pub fn parse_encoders(listing: &str) -> Vec<Encoder> {
    let mut found = Vec::new();
    for line in listing.lines() {
        let mut t = line.split_whitespace();
        let (Some(flags), Some(name)) = (t.next(), t.next()) else { continue };
        if !flags.starts_with('V') {
            continue;
        }
        if let Some(e) = Encoder::ALL.iter().copied().find(|e| e.name() == name) {
            found.push(e);
        }
    }
    found
}

static DETECTED: OnceLock<Encoders> = OnceLock::new();

/// The detected encoders, if [`detect_encoders`] has run (never blocks; for the UI).
pub fn known_encoders() -> Option<&'static Encoders> {
    DETECTED.get()
}

/// Detects the encoders in the background (the export dialog shows them when it opens).
pub fn warm() {
    if DETECTED.get().is_none() {
        std::thread::spawn(|| {
            detect_encoders();
        });
    }
}

/// Which encoders ffmpeg lists and, for the GPU ones, can actually open a device (a listed NVENC
/// still fails on a machine without an NVIDIA GPU). Runs ffmpeg once; cached for the session.
pub fn detect_encoders() -> &'static Encoders {
    DETECTED.get_or_init(|| {
        let Ok(out) = quiet_command("ffmpeg").args(["-hide_banner", "-encoders"]).stderr(Stdio::null()).output() else { return Encoders::software() };
        let listed = parse_encoders(&String::from_utf8_lossy(&out.stdout));
        Encoders { available: listed.into_iter().filter(|e| !e.is_gpu() || probe(*e)).collect() }
    })
}

fn probe(e: Encoder) -> bool {
    quiet_command("ffmpeg")
        .args(["-hide_banner", "-v", "error", "-f", "lavfi", "-i", "color=c=black:s=640x360:d=0.2:r=10", "-frames:v", "2", "-c:v", e.name(), "-f", "null", "-"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

// ------------------------------------------------------------------ presets

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Youtube1080,
    Youtube4k,
    /// Vertical 1080x1920 for Shorts / Reels.
    Shorts,
    Draft720,
}

impl Preset {
    pub const ALL: [Preset; 4] = [Preset::Youtube1080, Preset::Youtube4k, Preset::Shorts, Preset::Draft720];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Youtube1080 => "YouTube 1080p",
            Preset::Youtube4k => "YouTube 4K",
            Preset::Shorts => "Shorts / Reels 1080x1920",
            Preset::Draft720 => "Draft 720p",
        }
    }

    /// Target video bitrate, if the preset has one.
    fn bitrate_m(self) -> Option<u32> {
        match self {
            Preset::Youtube1080 => Some(16),
            Preset::Youtube4k => Some(45),
            Preset::Shorts => Some(12),
            Preset::Draft720 => None,
        }
    }

    fn scale(self) -> &'static str {
        match self {
            Preset::Youtube1080 => "scale=-2:1080",
            Preset::Youtube4k => "scale=-2:2160",
            Preset::Shorts => "scale=1080:1920:force_original_aspect_ratio=decrease,pad=1080:1920:(ow-iw)/2:(oh-ih)/2",
            Preset::Draft720 => "scale=-2:720",
        }
    }

    fn wants_hevc(self) -> bool {
        self == Preset::Youtube4k
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    Draft,
    #[default]
    Standard,
    High,
}

impl Quality {
    fn idx(self) -> usize {
        self as usize
    }
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// The ffmpeg output arguments (after the inputs) for `preset` at `quality`: scale filter, video
/// encoder, rate control, audio and `+faststart`. Add `-af loudnorm_filter(..)` and the output path
/// yourself.
pub fn preset_args(preset: Preset, quality: Quality, enc: &Encoders) -> Vec<String> {
    let mut a = s(&["-vf", preset.scale()]);
    a.extend(video_codec_args(preset, quality, enc));
    a.extend(s(&["-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart"]));
    a
}

/// Only the video encoder's arguments of [`preset_args`] (codec, rate control, tag and pixel
/// format), for an export that brings its own filters and audio.
pub fn video_codec_args(preset: Preset, quality: Quality, enc: &Encoders) -> Vec<String> {
    let encoder = enc.pick(preset.wants_hevc());
    let q = quality.idx();
    let mut a = s(&["-c:v", encoder.name()]);
    let ten_bit = preset == Preset::Youtube4k && encoder.is_hevc();
    let mbit = preset.bitrate_m();
    match encoder {
        Encoder::H264Nvenc | Encoder::HevcNvenc => {
            let cq = ["32", "26", "22"][q];
            a.extend(s(&["-preset", "p4", "-tune", "hq", "-rc", "vbr", "-cq", cq]));
            match mbit {
                Some(m) => a.extend(s(&["-b:v", &format!("{m}M"), "-maxrate", &format!("{}M", m * 5 / 4), "-bufsize", &format!("{}M", m * 2)])),
                None => a.extend(s(&["-b:v", "0"])),
            }
            a.extend(s(&["-spatial-aq", "1"]));
        }
        Encoder::H264Qsv => {
            a.extend(s(&["-preset", "slow", "-global_quality", ["30", "25", "21"][q]]));
            if let Some(m) = mbit {
                a.extend(s(&["-maxrate", &format!("{}M", m * 5 / 4)]));
            }
        }
        Encoder::H264Amf => {
            let qp = ["30", "24", "20"][q];
            a.extend(s(&["-quality", "quality", "-rc", "cqp", "-qp_i", qp, "-qp_p", qp, "-qp_b", qp]));
        }
        Encoder::Libx264 => {
            a.extend(s(&["-preset", "slow", "-crf", ["24", "20", "18"][q]]));
            if let Some(m) = mbit {
                a.extend(s(&["-maxrate", &format!("{}M", m * 5 / 4), "-bufsize", &format!("{}M", m * 2)]));
            }
        }
        Encoder::Libx265 => {
            a.extend(s(&["-preset", "medium", "-crf", ["30", "24", "20"][q]]));
            if let Some(m) = mbit {
                a.extend(s(&["-maxrate", &format!("{}M", m * 5 / 4), "-bufsize", &format!("{}M", m * 2)]));
            }
        }
    }
    if encoder.is_hevc() {
        a.extend(s(&["-tag:v", "hvc1"]));
    }
    match (encoder, ten_bit) {
        (Encoder::HevcNvenc, true) => a.extend(s(&["-profile:v", "main10", "-pix_fmt", "p010le"])),
        (Encoder::Libx265, true) => a.extend(s(&["-profile:v", "main10", "-pix_fmt", "yuv420p10le"])),
        _ => a.extend(s(&["-pix_fmt", "yuv420p"])),
    }
    a
}

/// The encoders an export may use: everything detected, or only the software ones.
pub fn usable(gpu: bool) -> Encoders {
    let all = detect_encoders();
    if gpu {
        all.clone()
    } else {
        Encoders { available: all.available.iter().copied().filter(|e| !e.is_gpu()).collect() }
    }
}

/// EBU R128 loudness normalisation to `lufs` (YouTube plays at -14), true peak -1.5 dB.
pub fn loudnorm_filter(lufs: f64) -> String {
    format!("loudnorm=I={lufs}:TP=-1.5:LRA=11")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(a: &[String], pair: [&str; 2]) -> bool {
        a.windows(2).any(|w| w[0] == pair[0] && w[1] == pair[1])
    }

    fn gpu() -> Encoders {
        Encoders { available: vec![Encoder::H264Nvenc, Encoder::HevcNvenc, Encoder::Libx264, Encoder::Libx265] }
    }

    fn cpu() -> Encoders {
        Encoders { available: vec![Encoder::Libx264] }
    }

    #[test]
    fn parses_listing() {
        let out = " V....D h264_nvenc           NVIDIA NVENC H.264 encoder\n V....D libx264              libx264 H.264\n A....D aac                  AAC\n V....D libx265              libx265\n V....D mpeg4                MPEG-4\n";
        assert_eq!(parse_encoders(out), [Encoder::H264Nvenc, Encoder::Libx264, Encoder::Libx265]);
        assert!(parse_encoders("").is_empty());
    }

    #[test]
    fn youtube_1080_prefers_nvenc() {
        let a = preset_args(Preset::Youtube1080, Quality::Standard, &gpu());
        assert!(has(&a, ["-c:v", "h264_nvenc"]) && has(&a, ["-preset", "p4"]) && has(&a, ["-tune", "hq"]));
        assert!(has(&a, ["-rc", "vbr"]) && has(&a, ["-cq", "26"]) && has(&a, ["-b:v", "16M"]) && has(&a, ["-spatial-aq", "1"]));
        assert!(has(&a, ["-vf", "scale=-2:1080"]));
    }

    #[test]
    fn falls_back_to_libx264_slow() {
        for (q, crf) in [(Quality::Draft, "24"), (Quality::Standard, "20"), (Quality::High, "18")] {
            let a = preset_args(Preset::Youtube1080, q, &cpu());
            assert!(has(&a, ["-c:v", "libx264"]) && has(&a, ["-preset", "slow"]) && has(&a, ["-crf", crf]));
        }
        assert_eq!(Encoders::default().pick(false), Encoder::Libx264);
    }

    #[test]
    fn four_k_is_hevc_10bit_or_h264() {
        let a = preset_args(Preset::Youtube4k, Quality::Standard, &gpu());
        assert!(has(&a, ["-c:v", "hevc_nvenc"]) && has(&a, ["-b:v", "45M"]) && has(&a, ["-pix_fmt", "p010le"]) && has(&a, ["-tag:v", "hvc1"]));
        let sw = preset_args(Preset::Youtube4k, Quality::Standard, &Encoders::software());
        assert!(has(&sw, ["-c:v", "libx265"]) && has(&sw, ["-pix_fmt", "yuv420p10le"]));
        let h = preset_args(Preset::Youtube4k, Quality::Standard, &cpu());
        assert!(has(&h, ["-c:v", "libx264"]) && has(&h, ["-pix_fmt", "yuv420p"]) && has(&h, ["-vf", "scale=-2:2160"]));
    }

    #[test]
    fn shorts_and_draft() {
        let a = preset_args(Preset::Shorts, Quality::Standard, &gpu());
        assert!(a.iter().any(|x| x.starts_with("scale=1080:1920")) && has(&a, ["-b:v", "12M"]));
        let d = preset_args(Preset::Draft720, Quality::Draft, &gpu());
        assert!(has(&d, ["-b:v", "0"]) && has(&d, ["-cq", "32"]) && has(&d, ["-vf", "scale=-2:720"]));
    }

    #[test]
    fn other_gpu_encoders_and_loudnorm() {
        let qsv = Encoders { available: vec![Encoder::H264Qsv, Encoder::Libx264] };
        assert!(has(&preset_args(Preset::Draft720, Quality::High, &qsv), ["-global_quality", "21"]));
        let amf = Encoders { available: vec![Encoder::H264Amf] };
        assert!(has(&preset_args(Preset::Draft720, Quality::High, &amf), ["-rc", "cqp"]));
        assert_eq!(loudnorm_filter(-14.0), "loudnorm=I=-14:TP=-1.5:LRA=11");
        assert!(gpu().has_hevc() && !cpu().has_hevc());
        assert_eq!(Preset::ALL.len(), 4);
        assert!(!Preset::Shorts.label().is_empty());
    }

    #[test]
    fn codec_args_leave_filters_and_audio_out() {
        let v = video_codec_args(Preset::Youtube1080, Quality::High, &gpu());
        assert_eq!(&v[..2], ["-c:v", "h264_nvenc"]);
        assert!(!v.iter().any(|a| a == "-vf" || a == "-c:a" || a == "+faststart"));
        assert!(has(&v, ["-pix_fmt", "yuv420p"]));
    }
}

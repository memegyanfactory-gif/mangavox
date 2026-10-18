//! "Will it fit on this PC": hardware detection plus the pure rating / grouping logic of the
//! Model Hub (port of the helpers in ModelHubTab.tsx).
use mv_ai::{HubModelFile, HubSystemFit};

/// Total RAM (sysinfo) and best-effort GPU name + VRAM.
pub fn detect_system() -> HubSystemFit {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let ram_gb = sys.total_memory() as f64 / 1024f64.powi(3);
    let (gpu_name, vram_gb) = detect_gpu();
    HubSystemFit { vram_gb, ram_gb: (ram_gb * 10.0).round() / 10.0, gpu_name }
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let mut c = std::process::Command::new(cmd);
    c.args(args).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = c.output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn detect_gpu() -> (Option<String>, f64) {
    // NVIDIA: exact numbers.
    if let Some(o) = run("nvidia-smi", &["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"]) {
        if let Some(r) = parse_nvidia_smi(&o) {
            return r;
        }
    }
    #[cfg(windows)]
    {
        let name = run("wmic", &["path", "win32_VideoController", "get", "name"]).and_then(|o| parse_wmic_names(&o));
        // The registry holds the real 64-bit VRAM size (WMI's AdapterRAM caps at 4 GB).
        let reg = run("reg", &["query", r"HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}", "/s", "/v", "HardwareInformation.qwMemorySize"]).and_then(|o| parse_reg_qword(&o));
        if let Some(bytes) = reg {
            return (name, bytes as f64 / 1024f64.powi(3));
        }
        if let Some(o) = run("wmic", &["path", "win32_VideoController", "get", "AdapterRAM"]) {
            let max = o.lines().filter_map(|l| l.trim().parse::<u64>().ok()).max().unwrap_or(0);
            return (name, max as f64 / 1024f64.powi(3));
        }
        return (name, 0.0);
    }
    #[allow(unreachable_code)]
    (None, 0.0)
}

pub fn parse_nvidia_smi(out: &str) -> Option<(Option<String>, f64)> {
    let mut best: Option<(String, f64)> = None;
    for line in out.lines() {
        let (name, mib) = line.rsplit_once(',')?;
        let gb = mib.trim().parse::<f64>().ok()? / 1024.0;
        if best.as_ref().map(|b| gb > b.1).unwrap_or(true) {
            best = Some((name.trim().to_string(), gb));
        }
    }
    best.map(|(n, g)| (Some(n), g))
}

pub fn parse_reg_qword(out: &str) -> Option<u64> {
    out.lines()
        .filter(|l| l.contains("qwMemorySize"))
        .filter_map(|l| {
            let hex = l.split_whitespace().last()?.trim_start_matches("0x");
            u64::from_str_radix(hex, 16).ok()
        })
        .max()
}

pub fn parse_wmic_names(out: &str) -> Option<String> {
    let names: Vec<&str> = out.lines().map(str::trim).filter(|l| !l.is_empty() && !l.eq_ignore_ascii_case("name")).collect();
    // Prefer a discrete GPU over the integrated one.
    names.iter().find(|n| { let l = n.to_lowercase(); l.contains("nvidia") || l.contains("radeon rx") || l.contains("arc") }).or(names.first()).map(|s| s.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    Great,
    Ok,
    Slow,
    No,
}

impl Fit {
    pub fn text(self) -> &'static str {
        match self {
            Fit::Great => "Runs well",
            Fit::Ok => "Runs",
            Fit::Slow => "Runs slowly",
            Fit::No => "Too big",
        }
    }
    pub fn detail(self) -> &'static str {
        match self {
            Fit::Great => "The whole model fits on your graphics card.",
            Fit::Ok => "Part of the model runs on the CPU, so replies are slower.",
            Fit::Slow => "Runs on CPU memory — usable, but slow.",
            Fit::No => "Needs more memory than this PC has.",
        }
    }
}

pub fn fit_for(size_bytes: u64, fit: Option<&HubSystemFit>) -> Fit {
    let Some(fit) = fit else { return Fit::Ok };
    if size_bytes == 0 {
        return Fit::Ok;
    }
    let gb = size_bytes as f64 / 1024f64.powi(3);
    if fit.vram_gb > 0.0 && gb + 1.2 <= fit.vram_gb {
        Fit::Great
    } else if gb + 2.0 <= fit.vram_gb + fit.ram_gb * 0.5 {
        if fit.vram_gb > 0.0 { Fit::Ok } else { Fit::Slow }
    } else if gb + 2.0 <= fit.ram_gb * 0.85 {
        Fit::Slow
    } else {
        Fit::No
    }
}

/// (group title, is-recommended) for a quant option.
pub fn quant_group(f: &HubModelFile, recommended: Option<&str>) -> (&'static str, bool) {
    let b = f.bits.unwrap_or(0.0);
    if recommended == Some(f.path.as_str()) {
        return ("Balanced", true);
    }
    if b > 0.0 && b < 4.4 {
        ("Smaller", false)
    } else if b > 0.0 && b < 5.4 {
        ("Balanced", false)
    } else if b > 0.0 && b < 7.5 {
        ("Higher quality", false)
    } else {
        ("Best quality", false)
    }
}

pub fn pick_recommended(files: &[HubModelFile], fit: Option<&HubSystemFit>) -> Option<String> {
    let usable: Vec<&HubModelFile> = files.iter().filter(|f| fit_for(f.size, fit) != Fit::No).collect();
    for q in ["Q4_K_M", "Q4_K_XL", "Q4_K_S", "MXFP4", "IQ4_XS", "Q5_K_M", "Q3_K_M"] {
        if let Some(h) = usable.iter().copied().find(|f| f.quant.as_deref() == Some(q)).or_else(|| files.iter().find(|f| f.quant.as_deref() == Some(q))) {
            return Some(h.path.clone());
        }
    }
    usable.get(usable.len() / 2).copied().or_else(|| files.first()).map(|f| f.path.clone())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Filter {
    Fits,
    Moe,
    Vision,
    Coding,
    Reasoning,
    Distilled,
    Uncensored,
}

impl Filter {
    pub const ALL: [Filter; 7] = [Filter::Fits, Filter::Moe, Filter::Vision, Filter::Coding, Filter::Reasoning, Filter::Distilled, Filter::Uncensored];
    pub fn label(self) -> &'static str {
        match self {
            Filter::Fits => "Fits this PC",
            Filter::Moe => "Mixture of experts",
            Filter::Vision => "Vision",
            Filter::Coding => "Coding",
            Filter::Reasoning => "Reasoning",
            Filter::Distilled => "Distilled",
            Filter::Uncensored => "Uncensored",
        }
    }
}

/// Rough capability tags from the repo name + HF tags.
pub fn infer_tags(repo: &str, tags: &[String], pipeline: Option<&str>) -> Vec<Filter> {
    let s = format!("{repo} {}", tags.join(" ")).to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| s.contains(w));
    let mut out = vec![];
    let moe_a = regex_a_b(&s);
    if moe_a || has(&["moe", "mixtral"]) {
        out.push(Filter::Moe);
    }
    if has(&["vision", "-vl", "image-text", "llava", "gemma-3", "gemma-4", "gemma3", "gemma4", "pixtral", "multimodal"]) || pipeline == Some("image-text-to-text") {
        out.push(Filter::Vision);
    }
    if has(&["coder", "code", "devstral", "codestral"]) {
        out.push(Filter::Coding);
    }
    if has(&["r1", "reason", "think", "qwq", "gpt-oss", "qwen3", "magistral"]) {
        out.push(Filter::Reasoning);
    }
    if has(&["distill"]) {
        out.push(Filter::Distilled);
    }
    if has(&["abliterat", "uncensor", "dolphin", "lexi", "nsfw", "unfiltered", "heretic"]) {
        out.push(Filter::Uncensored);
    }
    out
}

/// Matches active-parameter markers like `-a3b` / `a22b` (mixture-of-experts naming).
fn regex_a_b(s: &str) -> bool {
    let b = s.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'a' && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            let mut j = i + 1;
            let start = j;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'.') {
                j += 1;
            }
            if j > start && j < b.len() && b[j] == b'b' && (j + 1 == b.len() || !b[j + 1].is_ascii_alphanumeric()) {
                return true;
            }
        }
    }
    false
}

/// "7B" style parameter count from a repo name.
pub fn params_from_name(repo: &str) -> Option<f64> {
    let b = repo.as_bytes();
    for i in 0..b.len() {
        if b[i].is_ascii_digit() && (i == 0 || !b[i - 1].is_ascii_alphanumeric() && b[i - 1] != b'.') {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'.') {
                j += 1;
            }
            if j < b.len() && (b[j] == b'b' || b[j] == b'B') && (j + 1 == b.len() || !b[j + 1].is_ascii_lowercase()) {
                return repo[i..j].parse().ok();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sys(vram: f64, ram: f64) -> HubSystemFit {
        HubSystemFit { vram_gb: vram, ram_gb: ram, gpu_name: None }
    }
    const GB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn fit_ratings() {
        let s = sys(8.0, 32.0);
        assert_eq!(fit_for(4 * GB, Some(&s)), Fit::Great);
        assert_eq!(fit_for(12 * GB, Some(&s)), Fit::Ok);
        assert_eq!(fit_for(40 * GB, Some(&s)), Fit::No);
        assert_eq!(fit_for(10 * GB, Some(&sys(0.0, 32.0))), Fit::Slow);
        assert_eq!(fit_for(30 * GB, Some(&sys(0.0, 32.0))), Fit::No);
        assert_eq!(fit_for(30 * GB, None), Fit::Ok);
    }

    #[test]
    fn recommended_and_groups() {
        let f = |q: &str, bits: f64, gb: u64| HubModelFile { path: format!("m-{q}.gguf"), size: gb * GB, quant: Some(q.into()), bits: Some(bits), parts: None };
        let files = vec![f("Q2_K", 2.6, 2), f("Q4_K_M", 4.8, 5), f("Q8_0", 8.5, 9)];
        let rec = pick_recommended(&files, Some(&sys(16.0, 32.0))).unwrap();
        assert_eq!(rec, "m-Q4_K_M.gguf");
        assert_eq!(quant_group(&files[0], Some(&rec)), ("Smaller", false));
        assert_eq!(quant_group(&files[1], Some(&rec)), ("Balanced", true));
        assert_eq!(quant_group(&files[2], Some(&rec)), ("Best quality", false));
    }

    #[test]
    fn tags_and_params() {
        let t = infer_tags("unsloth/Qwen3-30B-A3B-GGUF", &[], None);
        assert!(t.contains(&Filter::Moe) && t.contains(&Filter::Reasoning));
        assert!(infer_tags("x/dolphin-3-8b", &[], None).contains(&Filter::Uncensored));
        assert!(infer_tags("x/foo", &[], Some("image-text-to-text")).contains(&Filter::Vision));
        assert_eq!(params_from_name("bartowski/Llama-3.2-3B-Instruct-GGUF"), Some(3.0));
        assert_eq!(params_from_name("x/Qwen3.5-4B"), Some(4.0));
        assert_eq!(params_from_name("x/no-size"), None);
    }

    #[test]
    fn parses_hardware_output() {
        assert_eq!(parse_nvidia_smi("NVIDIA GeForce RTX 4070, 12282\n").unwrap().1.round(), 12.0);
        assert_eq!(parse_reg_qword("    HardwareInformation.qwMemorySize    REG_QWORD    0x300000000\n"), Some(0x3_0000_0000));
        assert_eq!(parse_wmic_names("Name\r\nIntel(R) UHD\r\nNVIDIA GeForce RTX 3060\r\n").as_deref(), Some("NVIDIA GeForce RTX 3060"));
        let s = detect_system();
        assert!(s.ram_gb > 0.0);
    }
}

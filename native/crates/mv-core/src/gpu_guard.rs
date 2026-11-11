//! Power supply protection for NVIDIA graphics cards (Settings → System).
//!
//! The PC has switched itself off during renders and voicing even with the card held to 224 W
//! and MangaVox pacing its GPU work to about 200 W (`limits::gpu_watts`, python/gpu_power.py).
//! What trips a power supply is the card's own millisecond spikes, which pacing cannot touch:
//! their height follows the clock and voltage the card runs at. So this caps both, with
//! NVIDIA's own tool: `nvidia-smi -lgc` (highest clock) and `-pl` (power limit).
//!
//! Both need administrator rights and last until the PC restarts, so turning it on asks Windows
//! once (UAC), applies them at once and registers a startup task ([`TASK`], run as SYSTEM at
//! start-up and one minute after sign-in, after tools such as MSI Afterburner set their own
//! limit) that applies them again. Turning it off restores the earlier power limit, frees the
//! clocks and removes the task. Reading the state needs no rights.
use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// The Windows scheduled task that applies the caps at every start.
pub const TASK: &str = "MangaVox GPU safety";
/// config.json key: what was applied, and the power limit to restore.
pub const CONFIG_KEY: &str = "system.gpuGuard";
/// The highest clock allowed under the cap (MHz) on fast cards: an RTX 3080 boosts to about
/// 2,000 MHz at about 1.08 V; at 1,600 MHz it runs near 0.8 V, which cuts its spikes the most
/// for the speed it costs (renders and voicing get roughly 15–25% slower).
pub const CLOCK_CAP_MHZ: u32 = 1600;
/// The power limit under the cap, as a share of the card's default limit (200 W of 320).
pub const WATTS_SHARE: f64 = 0.62;

/// The NVIDIA card, as nvidia-smi reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub name: String,
    /// Highest clock the card can run (MHz).
    pub max_clock: u32,
    pub min_watts: u32,
    pub default_watts: u32,
    /// The power limit in force now (watts).
    pub limit_watts: u32,
}

/// What turning protection on applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caps {
    pub clock_mhz: u32,
    pub watts: u32,
}

/// What is saved while protection is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub clock_mhz: u32,
    pub watts: u32,
    /// The power limit before protection was turned on, restored when it is turned off.
    pub previous_watts: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    /// `None`: no NVIDIA card (or no driver tools): nothing to protect here.
    pub card: Option<Card>,
    /// The startup task is installed.
    pub on: bool,
    /// The caps that are (when on) or would be applied.
    pub caps: Option<Caps>,
}

/// nvidia-smi from the NVIDIA driver (installed in System32), else from PATH.
pub fn smi() -> Option<PathBuf> {
    let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let sys = root.join("System32").join("nvidia-smi.exe");
    if sys.is_file() {
        return Some(sys);
    }
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("nvidia-smi.exe")).find(|f| f.is_file()))
}

fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// Parses one line of `--query-gpu=name,clocks.max.graphics,power.min_limit,power.default_limit,power.limit`.
pub fn parse_card(line: &str) -> Option<Card> {
    let p: Vec<&str> = line.split(',').map(str::trim).collect();
    if p.len() < 5 {
        return None;
    }
    let num = |s: &str| s.split_whitespace().next().and_then(|n| n.parse::<f64>().ok()).filter(|n| *n > 0.0).map(|n| n.round() as u32);
    Some(Card { name: p[0].to_string(), max_clock: num(p[1])?, min_watts: num(p[2])?, default_watts: num(p[3])?, limit_watts: num(p[4])? })
}

/// The first NVIDIA card (blocking: runs nvidia-smi).
pub fn card() -> Option<Card> {
    let smi = smi()?;
    let out = hidden(Command::new(smi).args(["--id=0", "--query-gpu=name,clocks.max.graphics,power.min_limit,power.default_limit,power.limit", "--format=csv,noheader,nounits"]))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_card(String::from_utf8_lossy(&out.stdout).lines().next()?)
}

/// The caps for a card: [`CLOCK_CAP_MHZ`] (or 80% of a slower card's top clock) and
/// [`WATTS_SHARE`] of its default power limit, never above a lower limit already in force,
/// never under its minimum.
pub fn caps_for(card: &Card) -> Caps {
    let clock = if card.max_clock >= CLOCK_CAP_MHZ * 6 / 5 { CLOCK_CAP_MHZ } else { (card.max_clock as f64 * 0.8 / 15.0).round() as u32 * 15 };
    let share = ((card.default_watts as f64 * WATTS_SHARE) / 5.0).round() as u32 * 5;
    let watts = share.min(card.limit_watts).max(card.min_watts);
    Caps { clock_mhz: clock.max(300), watts }
}

pub fn saved() -> Option<Saved> {
    crate::config::Config::open_default().get(CONFIG_KEY).and_then(|v| serde_json::from_value::<Saved>(v).ok()).filter(|s| s.watts > 0)
}

/// The startup task exists (no rights needed to look).
pub fn task_installed() -> bool {
    hidden(Command::new("schtasks").args(["/query", "/tn", TASK])).output().map(|o| o.status.success()).unwrap_or(false)
}

/// The state for the Settings row (blocking).
pub fn status() -> Status {
    let card = card();
    let on = card.is_some() && task_installed();
    let caps = match (on, saved(), &card) {
        (true, Some(s), _) => Some(Caps { clock_mhz: s.clock_mhz, watts: s.watts }),
        (_, _, Some(c)) => Some(caps_for(c)),
        _ => None,
    };
    Status { card, on, caps }
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The elevated script that applies `caps` now and registers [`TASK`] to apply them at every start.
pub fn on_script(smi: &str, caps: Caps) -> String {
    let (pl, lgc) = (format!("-pl {}", caps.watts), format!("-lgc 210,{}", caps.clock_mhz));
    format!(
        r#"$ErrorActionPreference = 'Stop'
$smi = {smi}
& $smi {pl} | Out-Null
if ($LASTEXITCODE -ne 0) {{ exit 11 }}
& $smi {lgc} | Out-Null
if ($LASTEXITCODE -ne 0) {{ exit 12 }}
$actions = @((New-ScheduledTaskAction -Execute $smi -Argument {pl_q}), (New-ScheduledTaskAction -Execute $smi -Argument {lgc_q}))
$logon = New-ScheduledTaskTrigger -AtLogOn
$logon.Delay = 'PT1M'
$triggers = @((New-ScheduledTaskTrigger -AtStartup), $logon)
$principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -ExecutionTimeLimit (New-TimeSpan -Minutes 2)
Register-ScheduledTask -TaskName {task} -Action $actions -Trigger $triggers -Principal $principal -Settings $settings -Description 'Caps the graphics card clock and power so its spikes stay within the power supply (MangaVox, Settings > System).' -Force | Out-Null
exit 0
"#,
        smi = ps_quote(smi),
        pl_q = ps_quote(&pl),
        lgc_q = ps_quote(&lgc),
        task = ps_quote(TASK),
    )
}

/// The elevated script that frees the clocks, restores `restore_watts` and removes [`TASK`].
pub fn off_script(smi: &str, restore_watts: u32) -> String {
    format!(
        r#"$smi = {smi}
& $smi -rgc | Out-Null
& $smi -pl {restore_watts} | Out-Null
Unregister-ScheduledTask -TaskName {task} -Confirm:$false -ErrorAction SilentlyContinue
if (Get-ScheduledTask -TaskName {task} -ErrorAction SilentlyContinue) {{ exit 13 }}
exit 0
"#,
        smi = ps_quote(smi),
        task = ps_quote(TASK),
    )
}

/// `-EncodedCommand` takes base64 of UTF-16LE.
pub fn encode_command(script: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// Runs `script` as administrator: Windows asks the user (UAC). Blocking until it finishes.
fn run_elevated(script: &str) -> Result<()> {
    let launcher = format!(
        "try {{ $p = Start-Process -FilePath powershell.exe -Verb RunAs -WindowStyle Hidden -Wait -PassThru -ArgumentList '-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-EncodedCommand','{}'; exit $p.ExitCode }} catch {{ exit 1223 }}",
        encode_command(script)
    );
    let st = hidden(Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &launcher])).status().context("could not start PowerShell")?;
    match st.code() {
        Some(0) => Ok(()),
        Some(1223) => bail!("Windows did not get permission, so nothing was changed."),
        Some(11) => bail!("The graphics card did not accept a power limit."),
        Some(12) => bail!("The graphics card did not accept a clock limit (it may need a newer NVIDIA driver)."),
        Some(13) => bail!("The startup task could not be removed."),
        c => bail!("Changing the graphics card settings failed (code {}).", c.map_or("none".into(), |c| c.to_string())),
    }
}

/// Turns protection on: one UAC prompt, caps applied now and at every start.
pub fn turn_on() -> Result<Caps> {
    let smi = smi().ok_or_else(|| anyhow!("No NVIDIA graphics card tools found."))?;
    let card = card().ok_or_else(|| anyhow!("No NVIDIA graphics card found."))?;
    let caps = caps_for(&card);
    // Keep the limit from before the first time (turning on again must not save our own cap).
    let previous = saved().map(|s| s.previous_watts).filter(|w| *w > 0).unwrap_or(card.limit_watts);
    run_elevated(&on_script(&smi.to_string_lossy(), caps))?;
    let s = Saved { clock_mhz: caps.clock_mhz, watts: caps.watts, previous_watts: previous };
    crate::config::Config::open_default().set(CONFIG_KEY, json!(s))?;
    crate::logging::global().log_with(crate::logging::Level::Info, "system", "gpu protection on", json!({ "card": card.name, "clockMhz": caps.clock_mhz, "watts": caps.watts, "previousWatts": previous }));
    refresh_risk();
    Ok(caps)
}

/// Turns protection off: one UAC prompt, the earlier power limit back, clocks free, task gone.
pub fn turn_off() -> Result<()> {
    let smi = smi().ok_or_else(|| anyhow!("No NVIDIA graphics card tools found."))?;
    let card = card();
    let restore = saved().map(|s| s.previous_watts).filter(|w| *w > 0).or_else(|| card.as_ref().map(|c| c.default_watts)).unwrap_or(0);
    if restore == 0 {
        bail!("Could not read the graphics card's power limit.");
    }
    run_elevated(&off_script(&smi.to_string_lossy(), restore))?;
    crate::config::Config::open_default().set(CONFIG_KEY, serde_json::Value::Null)?;
    crate::logging::global().log_with(crate::logging::Level::Info, "system", "gpu protection off", json!({ "restoredWatts": restore }));
    refresh_risk();
    Ok(())
}


// ---- Is this PC at risk? -----------------------------------------------------------------------

/// How far back power losses count as recent.
pub const LOSS_DAYS: u32 = 14;

/// The PC lost power lately and protection is off: Studio says so and offers to turn it on.
#[derive(Clone, Debug, PartialEq)]
pub struct Risk {
    pub card: String,
    /// Times Windows logged a power loss in the last [`LOSS_DAYS`] days.
    pub losses: u32,
    pub caps: Caps,
}

/// Counts the events in `wevtutil ... /f:xml` output.
pub fn count_events(xml: &str) -> u32 {
    xml.matches("<Event ").count() as u32
}

/// Times the PC went off without shutting down (Windows logs Kernel-Power 41 at the next start)
/// in the last `days` days. Reading the log needs no rights. Blocking.
pub fn power_losses(days: u32) -> u32 {
    let query = format!(
        "/q:*[System[Provider[@Name='Microsoft-Windows-Kernel-Power'] and (EventID=41) and TimeCreated[timediff(@SystemTime) <= {}]]]",
        days as u64 * 86_400_000
    );
    hidden(Command::new("wevtutil").args(["qe", "System", &query, "/c:200", "/f:xml"]))
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map_or(0, |o| count_events(&String::from_utf8_lossy(&o.stdout)))
}

/// The risk for a state and a number of recent power losses: none while protection is on,
/// without an NVIDIA card or when the PC never lost power.
pub fn risk_of(st: &Status, losses: u32) -> Option<Risk> {
    let card = st.card.as_ref()?;
    (!st.on && losses > 0).then(|| Risk { card: card.name.clone(), losses, caps: st.caps.unwrap_or_else(|| caps_for(card)) })
}

fn risk_cell() -> &'static std::sync::Mutex<Option<Risk>> {
    static R: std::sync::OnceLock<std::sync::Mutex<Option<Risk>>> = std::sync::OnceLock::new();
    R.get_or_init(Default::default)
}

/// Reads the risk again (blocking: nvidia-smi, the task list and the Windows event log).
pub fn refresh_risk() {
    let st = status();
    let risk = if st.card.is_some() && !st.on { risk_of(&st, power_losses(LOSS_DAYS)) } else { None };
    *risk_cell().lock().unwrap_or_else(|e| e.into_inner()) = risk;
}

/// The risk as last read; never blocks. The first call starts the read on its own thread, so it
/// answers `None` until that is done.
pub fn risk() -> Option<Risk> {
    static START: std::sync::Once = std::sync::Once::new();
    START.call_once(|| {
        std::thread::spawn(refresh_risk);
    });
    risk_cell().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rtx3080(limit: u32) -> Card {
        Card { name: "NVIDIA GeForce RTX 3080".into(), max_clock: 2100, min_watts: 100, default_watts: 320, limit_watts: limit }
    }

    #[test]
    fn reads_nvidia_smi_output() {
        let c = parse_card("NVIDIA GeForce RTX 3080, 2100, 100.00, 320.00, 224.00").unwrap();
        assert_eq!(c, rtx3080(224));
        assert!(parse_card("NVIDIA GeForce RTX 3080, [N/A], 100.00, 320.00, 224.00").is_none());
        assert!(parse_card("garbage").is_none());
    }

    #[test]
    fn caps_a_3080_to_1600_mhz_and_200_w() {
        assert_eq!(caps_for(&rtx3080(320)), Caps { clock_mhz: 1600, watts: 200 });
        // A lower limit already set (Afterburner) is kept.
        assert_eq!(caps_for(&rtx3080(180)), Caps { clock_mhz: 1600, watts: 180 });
        // Never under the card's minimum.
        assert_eq!(caps_for(&rtx3080(50)).watts, 100);
        // A slower card gets 80% of its top clock.
        let slow = Card { max_clock: 1500, default_watts: 120, min_watts: 60, limit_watts: 120, name: "x".into() };
        assert_eq!(caps_for(&slow), Caps { clock_mhz: 1200, watts: 75 });
    }

    #[test]
    fn a_pc_that_lost_power_is_at_risk_until_protection_is_on() {
        let card = rtx3080(224);
        let st = |on: bool| Status { card: Some(card.clone()), on, caps: Some(caps_for(&card)) };
        assert_eq!(risk_of(&st(false), 13), Some(Risk { card: card.name.clone(), losses: 13, caps: Caps { clock_mhz: 1600, watts: 200 } }));
        assert_eq!(risk_of(&st(false), 0), None, "never lost power");
        assert_eq!(risk_of(&st(true), 13), None, "already protected");
        assert_eq!(risk_of(&Status { card: None, on: false, caps: None }, 13), None, "no NVIDIA card");
        assert_eq!(count_events("<Event xmlns='x'><System/></Event><Event xmlns='x'><EventData/></Event>"), 2);
        assert_eq!(count_events(""), 0);
    }

    #[test]
    fn scripts_quote_paths_and_name_the_task() {
        let on = on_script(r"C:\Windows\System32\nvidia-smi.exe", Caps { clock_mhz: 1600, watts: 200 });
        assert!(on.contains(r"$smi = 'C:\Windows\System32\nvidia-smi.exe'"));
        assert!(on.contains("& $smi -pl 200") && on.contains("& $smi -lgc 210,1600"));
        assert!(on.contains("-Argument '-lgc 210,1600'") && on.contains("-TaskName 'MangaVox GPU safety'"));
        let off = off_script(r"C:\it's\nvidia-smi.exe", 224);
        assert!(off.contains(r"'C:\it''s\nvidia-smi.exe'") && off.contains("-rgc") && off.contains("-pl 224"));
    }

    /// PowerShell's parser accepts both scripts (parsed only, never run: running them as an
    /// administrator would change the graphics card).
    #[cfg(windows)]
    #[test]
    fn powershell_parses_both_scripts() {
        for script in [on_script(r"C:\Windows\System32\nvidia-smi.exe", Caps { clock_mhz: 1600, watts: 200 }), off_script(r"C:\Windows\System32\nvidia-smi.exe", 224)] {
            let check = format!(
                "$e = $null; [void][System.Management.Automation.Language.Parser]::ParseInput([Text.Encoding]::Unicode.GetString([Convert]::FromBase64String('{}')), [ref]$null, [ref]$e); if ($e.Count) {{ $e | ForEach-Object {{ $_.Message }}; exit 1 }}",
                encode_command(&script)
            );
            let out = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &check]).output().unwrap();
            assert!(out.status.success(), "{}\n{script}", String::from_utf8_lossy(&out.stdout));
        }
    }

    #[test]
    fn encodes_utf16_base64_like_powershell() {
        // [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes('exit 0'))
        assert_eq!(encode_command("exit 0"), "ZQB4AGkAdAAgADAA");
        assert_eq!(encode_command("a"), "YQA=");
        assert_eq!(encode_command(""), "");
    }
}

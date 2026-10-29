//! Windows Defender Firewall for Phone access. Windows blocks inbound connections to a new exe
//! until the user answers its "allow access" prompt (and clicking Cancel there leaves *block*
//! rules behind, which beat any allow rule). The Mobile page uses this module to check for and,
//! when the user clicks a button, add one narrow allow rule: TCP inbound to this exe on the
//! Phone access ports, Private networks only. Adding or removing it needs admin rights, so those
//! run through one UAC prompt; the checks are read-only and need none.
//!
//! Every function here runs a short child process (netsh / PowerShell, 0.3-1 s; the elevated
//! ones wait for the UAC prompt): call them off the UI thread.
//!
//! The rule is keyed to the exe path, so an in-place update keeps it; a dev build and the
//! installed app share the rule name, and adding from one points the rule at that one.
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::process::Command;

/// The firewall rule's (display) name, as `netsh ... name=` matches it.
pub const RULE_NAME: &str = "MangaVox phone access";
/// The ports the server tries, first free one wins (see `server`).
pub const PORT_FIRST: u16 = 8790;
pub const PORT_LAST: u16 = 8810;

/// Is our allow rule there, enabled, and does it let phones reach this exe on Private networks?
pub fn rule_present() -> bool {
    let Some((found, out)) = netsh(&format!("advfirewall firewall show rule name=\"{RULE_NAME}\" verbose")) else {
        return false;
    };
    found && rule_ok_in(&out, &exe_path())
}

/// Names of enabled inbound firewall rules that block phones from reaching this exe on a Private
/// network even with our rule in place: block rules for this exe (left by a cancelled Windows
/// prompt) and catch-all block rules (no program, all ports). Empty on a non-English Windows,
/// whose netsh output this does not read.
pub fn blocking_rules() -> Vec<String> {
    match netsh("advfirewall firewall show rule name=all dir=in verbose") {
        Some((true, out)) => blocking_in(&out, &exe_path()).into_iter().map(|r| r.name).collect(),
        _ => Vec::new(),
    }
}

/// Adds (or replaces) our allow rule through one UAC prompt, first deleting the block rules a
/// cancelled Windows prompt left for this exe. Blocks until done. `Err` carries a message for the
/// user (prompt declined, or the rule did not appear).
pub fn add_rule_elevated() -> Result<(), String> {
    let exe = exe_path().display().to_string();
    if exe.contains(['"', '%']) {
        return Err(format!("Can't add a firewall rule for this path automatically: {exe}"));
    }
    let netsh = netsh_exe().display().to_string();
    let mut cmds = Vec::new();
    if let Some((true, out)) = self::netsh("advfirewall firewall show rule name=all dir=in verbose") {
        for r in blocking_in(&out, &exe_path()) {
            // Only rules tied to this exe (and whose name survived netsh's code page); never
            // someone else's catch-all rule.
            if r.program.is_some() && r.name.is_ascii() && !r.name.contains(['"', '%']) {
                cmds.push(format!("\"{netsh}\" advfirewall firewall delete rule name=\"{}\" dir=in program=\"{exe}\"", r.name));
            }
        }
    }
    cmds.push(format!("\"{netsh}\" advfirewall firewall delete rule name=\"{RULE_NAME}\""));
    cmds.push(format!("\"{netsh}\" {}", add_rule_args(&exe)));
    run_elevated(&cmd_exe(), &cmd_line(&cmds))?;
    if rule_present() {
        Ok(())
    } else {
        Err("Windows didn't add the firewall rule.".into())
    }
}

/// Deletes our rule through one UAC prompt (for an explicit "Remove" button). Blocks until done.
pub fn remove_rule_elevated() -> Result<(), String> {
    if !rule_exists() {
        return Ok(());
    }
    run_elevated(&netsh_exe(), &format!("advfirewall firewall delete rule name=\"{RULE_NAME}\""))?;
    if rule_exists() {
        Err("Windows didn't remove the firewall rule.".into())
    } else {
        Ok(())
    }
}

/// Tries to delete our rule without elevation (works only when this process already runs as
/// admin). True when no rule is left.
pub fn remove_rule_quiet() -> bool {
    let _ = netsh(&format!("advfirewall firewall delete rule name=\"{RULE_NAME}\""));
    !rule_exists()
}

/// Is the network phones would use set to Private? `None` when Windows can't say (no network, or
/// the query failed). Uses the server's LAN address when it is running; a Domain network counts
/// as not Private (the rule covers Private only).
pub fn network_is_private() -> Option<bool> {
    network_is_private_for(crate::state::status().lan_ip)
}

/// [`network_is_private`] for the network that carries `lan_ip` (or, without one or when no
/// network has it, whether any connected network is Private).
pub fn network_is_private_for(lan_ip: Option<IpAddr>) -> Option<bool> {
    let script = "Get-NetConnectionProfile | ForEach-Object { $ips = (Get-NetIPAddress -InterfaceIndex $_.InterfaceIndex \
                  -AddressFamily IPv4 -ErrorAction SilentlyContinue).IPAddress -join ','; \"$([string]$_.NetworkCategory)|$ips\" }";
    let out = hidden(Command::new(powershell_exe()).args(["-NoProfile", "-NonInteractive", "-Command", script])).output().ok()?;
    private_in(&parse_profiles(&String::from_utf8_lossy(&out.stdout)), lan_ip)
}

/// Opens Settings → Network & internet, whose top card links to the current network's
/// properties (Public / Private).
pub fn open_network_settings() {
    let _ = hidden(Command::new(system32().join("cmd.exe")).args(["/d", "/c", "start", "", "ms-settings:network-status"])).spawn();
}

// --- netsh output ------------------------------------------------------------------------

/// One rule from `netsh advfirewall firewall show rule ... verbose` (English labels).
#[derive(Clone, Debug, Default, PartialEq)]
struct Rule {
    name: String,
    enabled: bool,
    inbound: bool,
    allow: bool,
    profiles: String,
    protocol: String,
    local_port: String,
    remote_ip: String,
    /// `None` for "Any" / no program line.
    program: Option<String>,
    service: Option<String>,
}

fn parse_rules(out: &str) -> Vec<Rule> {
    let mut rules: Vec<Rule> = Vec::new();
    for line in out.lines() {
        let Some((key, val)) = line.split_once(':') else { continue };
        let (key, val) = (key.trim(), val.trim());
        if key == "Rule Name" {
            rules.push(Rule { name: val.to_string(), local_port: "Any".into(), remote_ip: "Any".into(), protocol: "Any".into(), ..Default::default() });
            continue;
        }
        let Some(r) = rules.last_mut() else { continue };
        let any = |v: &str| (!v.is_empty() && !v.eq_ignore_ascii_case("Any")).then(|| v.to_string());
        match key {
            "Enabled" => r.enabled = val.eq_ignore_ascii_case("Yes"),
            "Direction" => r.inbound = val.eq_ignore_ascii_case("In"),
            "Action" => r.allow = val.eq_ignore_ascii_case("Allow"),
            "Profiles" => r.profiles = val.to_string(),
            "Protocol" => r.protocol = val.to_string(),
            "LocalPort" => r.local_port = val.to_string(),
            "RemoteIP" => r.remote_ip = val.to_string(),
            "Program" => r.program = any(val),
            "Service" => r.service = any(val),
            _ => {}
        }
    }
    rules
}

/// Our rule's `show rule name=... verbose` output (already known to match the name) passes.
/// Output netsh printed in another language has no labels to read: the name matched, trust it.
fn rule_ok_in(out: &str, exe: &std::path::Path) -> bool {
    let rules = parse_rules(out);
    rules.is_empty()
        || rules.iter().any(|r| {
            r.enabled
                && r.inbound
                && r.allow
                && covers_private(&r.profiles)
                && is_tcp(&r.protocol)
                && (PORT_FIRST..=PORT_LAST).all(|p| port_in(&r.local_port, p))
                && r.program.as_deref().is_some_and(|p| same_program(p, exe))
        })
}

fn blocking_in(out: &str, exe: &std::path::Path) -> Vec<Rule> {
    parse_rules(out)
        .into_iter()
        .filter(|r| {
            r.enabled
                && r.inbound
                && !r.allow
                && r.service.is_none()
                && covers_private(&r.profiles)
                && is_tcp(&r.protocol)
                && (PORT_FIRST..=PORT_LAST).any(|p| port_in(&r.local_port, p))
                && match &r.program {
                    Some(p) => same_program(p, exe),
                    None => remote_covers_lan(&r.remote_ip),
                }
        })
        .collect()
}

fn covers_private(profiles: &str) -> bool {
    profiles.split(',').any(|p| p.trim().eq_ignore_ascii_case("Private") || p.trim().eq_ignore_ascii_case("Any"))
}

fn is_tcp(protocol: &str) -> bool {
    protocol.eq_ignore_ascii_case("TCP") || protocol.eq_ignore_ascii_case("Any")
}

/// `spec` is "Any" or a comma list of ports and `a-b` ranges.
fn port_in(spec: &str, port: u16) -> bool {
    spec.split(',').map(str::trim).any(|item| {
        if item.eq_ignore_ascii_case("Any") {
            return true;
        }
        match item.split_once('-') {
            Some((a, b)) => matches!((a.trim().parse::<u16>(), b.trim().parse::<u16>()), (Ok(a), Ok(b)) if a <= port && port <= b),
            None => item.parse::<u16>() == Ok(port),
        }
    })
}

/// Does a block rule's RemoteIP list include typical home LAN addresses?
fn remote_covers_lan(spec: &str) -> bool {
    let lan = [Ipv4Addr::new(192, 168, 1, 2), Ipv4Addr::new(10, 0, 0, 2), Ipv4Addr::new(172, 16, 0, 2)];
    spec.split(',').map(str::trim).any(|item| {
        if ["Any", "LocalSubnet"].iter().any(|k| item.eq_ignore_ascii_case(k)) {
            return true;
        }
        let range = if let Some((a, b)) = item.split_once('-') {
            a.trim().parse::<Ipv4Addr>().ok().zip(b.trim().parse::<Ipv4Addr>().ok()).map(|(a, b)| (u32::from(a), u32::from(b)))
        } else if let Some((a, bits)) = item.split_once('/') {
            // netsh prints masks as either /24 or /255.255.255.0.
            let bits = bits.parse::<u32>().ok().or_else(|| bits.parse::<Ipv4Addr>().ok().map(|m| u32::from(m).count_ones()));
            a.parse::<Ipv4Addr>().ok().zip(bits.filter(|b| *b <= 32)).map(|(a, bits)| {
                let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
                (u32::from(a) & mask, (u32::from(a) & mask) | !mask)
            })
        } else {
            item.parse::<Ipv4Addr>().ok().map(|a| (u32::from(a), u32::from(a)))
        };
        range.is_some_and(|(lo, hi)| lan.iter().any(|ip| (lo..=hi).contains(&u32::from(*ip))))
    })
}

/// Case-insensitive path match after `%VAR%` expansion. netsh prints in the console code page, so
/// a non-ASCII exe path can't be compared reliably: then the file name has to do.
fn same_program(rule: &str, exe: &std::path::Path) -> bool {
    let rule = expand_env(rule);
    let exe_s = exe.display().to_string();
    if exe_s.is_ascii() {
        rule.eq_ignore_ascii_case(&exe_s)
    } else {
        let name = |s: &str| s.rsplit(['\\', '/']).next().unwrap_or("").to_ascii_lowercase();
        name(&rule) == name(&exe_s)
    }
}

fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) => {
                let var = &after[..j];
                match std::env::var(var) {
                    Ok(v) if !var.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(var);
                        out.push('%');
                    }
                }
                rest = &after[j + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

// --- network profiles --------------------------------------------------------------------

/// Lines of `Category|ip,ip` from the PowerShell query.
fn parse_profiles(out: &str) -> Vec<(String, Vec<Ipv4Addr>)> {
    out.lines()
        .filter_map(|l| {
            let (cat, ips) = l.trim().split_once('|')?;
            (!cat.is_empty()).then(|| (cat.trim().to_string(), ips.split(',').filter_map(|ip| ip.trim().parse().ok()).collect()))
        })
        .collect()
}

fn private_in(profiles: &[(String, Vec<Ipv4Addr>)], lan_ip: Option<IpAddr>) -> Option<bool> {
    if profiles.is_empty() {
        return None;
    }
    let is_private = |cat: &str| cat.eq_ignore_ascii_case("Private");
    if let Some(IpAddr::V4(ip)) = lan_ip {
        if let Some((cat, _)) = profiles.iter().find(|(_, ips)| ips.contains(&ip)) {
            return Some(is_private(cat));
        }
    }
    Some(profiles.iter().any(|(cat, _)| is_private(cat)))
}

// --- processes ---------------------------------------------------------------------------

fn exe_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("mangavox.exe"))
}

fn system32() -> PathBuf {
    let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    root.join("System32")
}

fn netsh_exe() -> PathBuf {
    system32().join("netsh.exe")
}

fn cmd_exe() -> PathBuf {
    system32().join("cmd.exe")
}

fn powershell_exe() -> PathBuf {
    system32().join(r"WindowsPowerShell\v1.0\powershell.exe")
}

fn add_rule_args(exe: &str) -> String {
    format!(
        "advfirewall firewall add rule name=\"{RULE_NAME}\" dir=in action=allow program=\"{exe}\" protocol=TCP \
         localport={PORT_FIRST}-{PORT_LAST} profile=private enable=yes"
    )
}

/// `cmd.exe` parameters running `cmds` in order (`/s`: cmd strips only the outer quotes and
/// leaves the quoted netsh arguments alone).
fn cmd_line(cmds: &[String]) -> String {
    format!("/d /s /c \"{}\"", cmds.join(" & "))
}

fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Runs netsh with `args` passed verbatim (netsh parses its own quotes). `(exit code 0, stdout)`.
fn netsh(args: &str) -> Option<(bool, String)> {
    let mut cmd = Command::new(netsh_exe());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.raw_arg(args);
    }
    #[cfg(not(windows))]
    cmd.args(args.split(' '));
    let out = hidden(&mut cmd).output().ok()?;
    Some((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned()))
}

fn rule_exists() -> bool {
    matches!(netsh(&format!("advfirewall firewall show rule name=\"{RULE_NAME}\"")), Some((true, _)))
}

/// Starts `file params` elevated ("runas": the UAC prompt), hidden, and waits for it to exit.
/// Returns its exit code.
#[cfg(windows)]
fn run_elevated(file: &std::path::Path, params: &str) -> Result<u32, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_CANCELLED, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let (verb, file_w, params_w, dir_w) = (wide("runas".as_ref()), wide(file.as_os_str()), wide(params.as_ref()), wide(system32().as_os_str()));
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file_w.as_ptr();
    info.lpParameters = params_w.as_ptr();
    info.lpDirectory = dir_w.as_ptr();
    info.nShow = SW_HIDE;
    // SAFETY: `info` and the strings it points to outlive the call.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(if unsafe { GetLastError() } == ERROR_CANCELLED {
            "Windows didn't get permission (the prompt was declined).".into()
        } else {
            format!("Couldn't ask Windows for permission ({}).", std::io::Error::last_os_error())
        });
    }
    if info.hProcess.is_null() {
        return Ok(0);
    }
    // SAFETY: `hProcess` is a process handle we own (SEE_MASK_NOCLOSEPROCESS) and close once.
    unsafe {
        let waited = WaitForSingleObject(info.hProcess, 120_000);
        let mut code = 1u32;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        if waited != WAIT_OBJECT_0 {
            return Err("Windows took too long to change the firewall.".into());
        }
        Ok(code)
    }
}

#[cfg(not(windows))]
fn run_elevated(_file: &std::path::Path, _params: &str) -> Result<u32, String> {
    Err("Only needed on Windows.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const EXE: &str = r"C:\Users\me\AppData\Local\Programs\MangaVox\mangavox.exe";

    const OURS: &str = "
Rule Name:                            MangaVox phone access
----------------------------------------------------------------------
Enabled:                              Yes
Direction:                            In
Profiles:                             Private
Grouping:
LocalIP:                              Any
RemoteIP:                             Any
Protocol:                             TCP
LocalPort:                            8790-8810
RemotePort:                           Any
Edge traversal:                       No
Program:                              C:\\Users\\me\\AppData\\Local\\Programs\\MangaVox\\mangavox.exe
InterfaceTypes:                       Any
Security:                             NotRequired
Rule source:                          Local Setting
Action:                               Allow
Ok.
";

    #[test]
    fn our_rule_is_recognised() {
        assert!(rule_ok_in(OURS, Path::new(EXE)));
        // Case differences in the path don't matter.
        assert!(rule_ok_in(OURS, Path::new(&EXE.to_lowercase())));
        // Pointing at another build (dev vs installed) does.
        assert!(!rule_ok_in(OURS, Path::new(r"D:\src\native\target\debug\mangavox.exe")));
        assert!(!rule_ok_in(&OURS.replace("Enabled:                              Yes", "Enabled: No"), Path::new(EXE)));
        assert!(!rule_ok_in(&OURS.replace("Profiles:                             Private", "Profiles: Public"), Path::new(EXE)));
        assert!(!rule_ok_in(&OURS.replace("8790-8810", "8790"), Path::new(EXE)));
        assert!(!rule_ok_in(&OURS.replace("Action:                               Allow", "Action: Block"), Path::new(EXE)));
        assert!(rule_ok_in(&OURS.replace("Profiles:                             Private", "Profiles: Domain,Private,Public"), Path::new(EXE)));
    }

    #[test]
    fn localised_output_trusts_the_name_match() {
        let de = "\nRegelname:                            MangaVox phone access\n----\nAktiviert:                            Ja\nRichtung:                             Eingehend\nOK.\n";
        assert!(parse_rules(de).is_empty());
        assert!(rule_ok_in(de, Path::new(EXE)));
    }

    #[test]
    fn finds_block_rules() {
        let out = format!(
            "{OURS}
Rule Name:                            MangaVox
----------------------------------------------------------------------
Enabled:                              Yes
Direction:                            In
Profiles:                             Private
RemoteIP:                             Any
Protocol:                             TCP
LocalPort:                            Any
Program:                              %LOCALAPPDATA%\\Programs\\MangaVox\\mangavox.exe
Action:                               Block

Rule Name:                            MangaVox
----------------------------------------------------------------------
Enabled:                              Yes
Direction:                            In
Profiles:                             Public
Protocol:                             TCP
LocalPort:                            Any
Program:                              {EXE}
Action:                               Block

Rule Name:                            codex_sandbox_offline_block_inbound
----------------------------------------------------------------------
Enabled:                              Yes
Direction:                            In
Profiles:                             Domain,Private,Public
RemoteIP:                             0.0.0.0-126.255.255.255,128.0.0.0-255.255.255.255,::-::,::2-ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff
Protocol:                             Any
Action:                               Block

Rule Name:                            Block one remote host
----------------------------------------------------------------------
Enabled:                              Yes
Direction:                            In
Profiles:                             Domain,Private,Public
RemoteIP:                             203.0.113.7/255.255.255.255
Protocol:                             Any
Action:                               Block

Rule Name:                            Some service block
----------------------------------------------------------------------
Enabled:                              Yes
Direction:                            In
Profiles:                             Domain,Private,Public
Protocol:                             TCP
Program:                              C:\\WINDOWS\\system32\\svchost.exe
Service:                              foo
Action:                               Block

Rule Name:                            Other app
----------------------------------------------------------------------
Enabled:                              Yes
Direction:                            In
Profiles:                             Private
Protocol:                             TCP
Program:                              C:\\Other\\other.exe
Action:                               Block

Rule Name:                            Disabled
----------------------------------------------------------------------
Enabled:                              No
Direction:                            In
Profiles:                             Private
Protocol:                             TCP
Action:                               Block

Rule Name:                            UDP only
----------------------------------------------------------------------
Enabled:                              Yes
Direction:                            In
Profiles:                             Private
Protocol:                             UDP
Action:                               Block
"
        );
        std::env::set_var("LOCALAPPDATA", r"C:\Users\me\AppData\Local");
        let names: Vec<String> = blocking_in(&out, Path::new(EXE)).into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["MangaVox", "codex_sandbox_offline_block_inbound"]);
    }

    #[test]
    fn ports_and_ranges() {
        assert!(port_in("Any", 8790));
        assert!(port_in("80,8790-8810", 8800));
        assert!(port_in("8795", 8795));
        assert!(!port_in("80,443", 8790));
        assert!(remote_covers_lan("LocalSubnet"));
        assert!(remote_covers_lan("192.168.0.0/16"));
        assert!(remote_covers_lan("192.168.1.0/255.255.255.0"));
        assert!(!remote_covers_lan("203.0.113.0/24"));
        assert!(!remote_covers_lan("127.0.0.1"));
    }

    #[test]
    fn network_profiles() {
        let out = "Public|192.168.0.156\r\nPrivate|10.0.0.5,10.0.0.6\r\nDomainAuthenticated|\r\n\r\n";
        let p = parse_profiles(out);
        assert_eq!(p.len(), 3);
        assert_eq!(private_in(&p, Some("192.168.0.156".parse().unwrap())), Some(false));
        assert_eq!(private_in(&p, Some("10.0.0.6".parse().unwrap())), Some(true));
        assert_eq!(private_in(&p, None), Some(true));
        assert_eq!(private_in(&parse_profiles("Public|1.2.3.4\nDomainAuthenticated|"), None), Some(false));
        assert_eq!(private_in(&[], None), None);
    }

    #[test]
    fn add_command_line() {
        let cmd = cmd_line(&[format!("\"netsh.exe\" {}", add_rule_args(EXE))]);
        assert_eq!(
            cmd,
            format!(
                "/d /s /c \"\"netsh.exe\" advfirewall firewall add rule name=\"MangaVox phone access\" dir=in action=allow \
                 program=\"{EXE}\" protocol=TCP localport=8790-8810 profile=private enable=yes\""
            )
        );
    }

    /// Read-only against the real firewall: the same `cmd /s /c` quoting the elevated add uses,
    /// with `show` commands; and the live queries don't fail.
    #[cfg(windows)]
    #[test]
    fn live_read_only_queries() {
        use std::os::windows::process::CommandExt;
        let netsh = netsh_exe().display().to_string();
        let line = cmd_line(&[
            format!("\"{netsh}\" advfirewall firewall show rule name=\"No such rule, MangaVox test\""),
            format!("\"{netsh}\" advfirewall firewall show rule name=\"Core Networking - DNS (UDP-Out)\""),
        ]);
        let out = hidden(Command::new(cmd_exe()).raw_arg(&line)).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{line}\n{text}");
        assert!(text.contains("Core Networking - DNS (UDP-Out)") || !text.contains("Rule Name"), "{text}");
        let _ = rule_present();
        eprintln!("blocking rules: {:?}", blocking_rules());
        eprintln!("network private: {:?}", network_is_private_for(None));
    }
}

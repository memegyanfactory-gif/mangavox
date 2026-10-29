//! The desktop Mobile page: the Phone access switch, the single-use pairing QR code (renewed when
//! it expires and right after a phone pairs), the address, setup steps and the paired phones.
use std::sync::Mutex;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius, ramp, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{
    btn, confirm_inline, empty_view, glyphs, inline_alert, ph, ph_icon, pulse_dot, section_header, size, spinner, switch, AlertKind, ButtonKind, Ph, RampExt as _, Tokens,
    Weight, MONO_FONT,
};

use crate::auth::{self, Device};
use crate::state::{self, Status};

const SUBTITLE: &str = "Read, listen, generate and control MangaVox from your phone on the same Wi-Fi.";
/// The QR code's side in px.
const QR_PX: f32 = 260.;
/// Turning on that has not produced a status by then counts as failed.
const START_TIMEOUT: Duration = Duration::from_secs(15);

/// The pairing code on screen. Shared by every Mobile page (one can sit in Back history), so a
/// page that is not visible never replaces the code the visible one shows.
struct Code {
    code: String,
    issued: Instant,
    /// Paired phones when the code was issued: one more means it was used.
    devices: usize,
}

static CODE: Mutex<Option<Code>> = Mutex::new(None);

/// The current code and when it was issued; issues a new one when there is none, it expired, a
/// phone paired with it, or `force`.
fn current_code(devices: usize, force: bool) -> Option<(String, Instant)> {
    let mut cur = CODE.lock().unwrap_or_else(|e| e.into_inner());
    let ttl = Duration::from_secs(auth::PAIR_TTL_SECS);
    let stale = match cur.as_mut() {
        None => true,
        Some(c) => {
            let used = devices > c.devices;
            // A phone removed: remember the lower count so the next pairing still renews.
            c.devices = c.devices.min(devices);
            force || used || c.issued.elapsed() >= ttl
        }
    };
    if stale {
        let code = auth::new_pair_code();
        *cur = (!code.is_empty()).then(|| Code { code, issued: Instant::now(), devices });
    }
    cur.as_ref().map(|c| (c.code.clone(), c.issued))
}

fn forget_code() {
    *CODE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub struct MobilePage {
    enabled: bool,
    status: Status,
    devices: Vec<Device>,
    epoch: u64,
    ticks: u64,
    /// Turned on and waiting for the server to report.
    starting: Option<Instant>,
    /// Turned on, but the server never reported (no status, no error).
    timed_out: bool,
    /// `state::epoch()` when the server was last asked to start.
    start_epoch: u64,
    confirm_remove_all: bool,
    /// The last connection check (firewall, network type), `None` until the first finishes.
    checks: Option<Checks>,
    checking: bool,
    /// Adding the firewall rule (waiting on the UAC prompt).
    fw_busy: bool,
    fw_error: Option<String>,
    _poll: Task<()>,
}

/// What can stop a phone from reaching this PC, read off the UI thread (netsh / PowerShell).
#[derive(Clone, Debug, Default)]
struct Checks {
    rule: bool,
    blocking: Vec<String>,
    private: Option<bool>,
}

impl MobilePage {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Once a second while the page is alive: the countdown, a renewed code, status and phones.
        let poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if this.update(cx, |p, cx| p.tick(cx)).is_err() {
                break;
            }
        });
        let mut page = Self {
            enabled: state::settings().enabled,
            status: state::status(),
            devices: vec![],
            epoch: state::epoch(),
            ticks: 0,
            starting: None,
            timed_out: false,
            start_epoch: 0,
            confirm_remove_all: false,
            checks: None,
            checking: false,
            fw_busy: false,
            fw_error: None,
            _poll: poll,
        };
        page.devices = auth::devices();
        page.devices.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        if page.enabled && !page.status.running && page.status.error.is_none() {
            page.starting = Some(Instant::now());
            page.start_epoch = page.epoch;
        }
        if page.enabled {
            page.run_checks(cx);
        }
        page
    }

    /// Re-reads the firewall rule, blocking rules and network type in the background.
    fn run_checks(&mut self, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        self.checking = true;
        cx.spawn(async move |this, cx| {
            let found = mv_ui::bg::blocking(|| Checks {
                rule: crate::firewall::rule_present(),
                blocking: crate::firewall::blocking_rules(),
                private: crate::firewall::network_is_private(),
            })
            .await;
            let _ = this.update(cx, |p, cx| {
                p.checking = false;
                p.checks = found.ok();
                cx.notify();
            });
        })
        .detach();
    }

    fn allow_firewall(&mut self, cx: &mut Context<Self>) {
        if self.fw_busy {
            return;
        }
        self.fw_busy = true;
        self.fw_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::blocking(crate::firewall::add_rule_elevated).await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |p, cx| {
                p.fw_busy = false;
                match r {
                    Ok(()) => toast::show(cx, Toast::success("Phones are allowed through Windows Firewall")),
                    Err(e) => p.fw_error = Some(e),
                }
                p.run_checks(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn open_network_settings(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let _ = mv_ui::bg::blocking(crate::firewall::open_network_settings).await;
            let _ = this.update(cx, |p, cx| p.run_checks(cx));
        })
        .detach();
    }

    fn refresh(&mut self) {
        self.epoch = state::epoch();
        self.status = state::status();
        self.devices = auth::devices();
        self.devices.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        // An error counts only once the server reported after this start (not a stale one).
        let reported = self.status.error.is_some() && self.epoch > self.start_epoch;
        if self.status.running || reported || !self.enabled {
            self.starting = None;
        }
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        self.ticks += 1;
        let was_running = self.status.running;
        // "Last active" moves without a change event, so phones are re-read now and then too.
        if state::epoch() != self.epoch || self.ticks % 15 == 0 {
            self.refresh();
        }
        // The network check needs the server's LAN address; a problem is re-checked now and
        // then, so fixing it in Windows Settings shows up here without a click.
        let problem = self.checks.as_ref().is_some_and(|c| !c.rule || !c.blocking.is_empty() || c.private == Some(false));
        if self.enabled && ((self.status.running && !was_running) || (problem && self.ticks % 20 == 0)) {
            self.run_checks(cx);
        }
        if self.status.running {
            current_code(self.devices.len(), false);
        }
        if self.starting.is_some_and(|t| t.elapsed() >= START_TIMEOUT) {
            self.starting = None;
            self.timed_out = !self.status.running;
        }
        cx.notify();
    }

    fn set_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        let mut s = state::settings();
        s.enabled = on;
        state::save_settings(&s);
        self.enabled = on;
        self.confirm_remove_all = false;
        self.timed_out = false;
        if on {
            self.starting = Some(Instant::now());
            self.start_epoch = state::epoch();
            crate::server::start();
            self.run_checks(cx);
        } else {
            self.starting = None;
            crate::server::stop();
            forget_code();
        }
        self.refresh();
        cx.notify();
    }

    fn retry(&mut self, cx: &mut Context<Self>) {
        crate::server::stop();
        self.timed_out = false;
        self.starting = Some(Instant::now());
        self.start_epoch = state::epoch();
        crate::server::start();
        self.refresh();
        cx.notify();
    }

    /// Why phone access is not running, once it has stopped trying.
    fn error(&self) -> Option<String> {
        if self.status.running || self.starting.is_some() {
            return None;
        }
        self.status.error.clone().or_else(|| self.timed_out.then(|| "The phone server did not start. Try again, or restart MangaVox.".to_string()))
    }

    fn new_code(&mut self, cx: &mut Context<Self>) {
        current_code(self.devices.len(), true);
        cx.notify();
    }

    fn remove(&mut self, id: String, name: String, cx: &mut Context<Self>) {
        auth::revoke(&id);
        self.refresh();
        toast::show(cx, Toast::success(format!("{name} can no longer connect")));
        cx.notify();
    }

    fn remove_all(&mut self, cx: &mut Context<Self>) {
        auth::revoke_all();
        self.confirm_remove_all = false;
        self.refresh();
        toast::show(cx, Toast::success("All phones were removed"));
        cx.notify();
    }

    // ── Parts ───────────────────────────────────────────────────────────────────────────────

    /// Off: what phone access does, and one big way to turn it on.
    fn intro(&self, cx: &mut Context<Self>) -> Div {
        let feature = |icon: Ph, title: &'static str, body: &'static str| {
            div()
                .flex()
                .items_start()
                .gap(px(space::MD))
                .w(px(380.))
                .child(tile(icon, 36.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(div().ramp(ramp::BODY_STRONG).text_color(Tokens::text_1()).child(title))
                        .child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_3()).child(body)),
                )
        };
        card()
            .p(px(space::XXXL))
            .gap(px(space::XXL))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::LG))
                    .child(tile(ph::DEVICE_MOBILE, 56.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(div().ramp(ramp::SUBTITLE).text_color(Tokens::text_1()).child("Use MangaVox from your phone"))
                            .child(
                                div()
                                    .max_w(px(560.))
                                    .ramp(ramp::BODY)
                                    .text_color(Tokens::text_2())
                                    .child("Phone access opens a small, private web app on this PC's Wi-Fi. Pair your phone once by scanning a code, then use it like an app."),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x(px(space::XXL))
                    .gap_y(px(space::LG))
                    .child(feature(glyphs::LIBRARY, "Read your library", "Every manga and chapter, with your place kept in sync."))
                    .child(feature(ph::HEADPHONES, "Listen and watch", "Narrations and recap videos, streamed or downloaded."))
                    .child(feature(glyphs::STUDIO, "Generate", "Start Studio narration and videos from the couch."))
                    .child(feature(ph::MONITOR, "Control this PC", "See this window and tap, scroll and type on it.")),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::LG))
                    .child(btn("turn-on", "Turn on phone access", ButtonKind::Primary).large().icon(ph::POWER).on_click(cx.listener(|this, _, _, cx| this.set_enabled(true, cx))))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(size::SM))
                            .text_color(Tokens::text_3())
                            .child(ph_icon(ph::LOCK_SIMPLE, Weight::Bold, Tokens::text_3(), icon_px::SM))
                            .child("Only phones that scan the code shown here can connect."),
                    ),
            )
    }

    /// The QR code card, or the reason there is none yet.
    fn qr_card(&self, cx: &mut Context<Self>) -> Div {
        let code = self.status.running.then(|| current_code(self.devices.len(), false)).flatten();
        let url = code.as_ref().and_then(|(c, _)| state::pair_url(c));
        let body = match (&url, &code) {
            (Some(url), Some((_, issued))) => {
                let left = auth::PAIR_TTL_SECS.saturating_sub(issued.elapsed().as_secs());
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(space::LG))
                    .child(div().p(px(space::MD)).rounded(px(radius::LG)).bg(Tokens::text_1()).shadow_md().child(crate::qr::qr_code(url, QR_PX)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::SM))
                            .child(ph_icon(ph::CLOCK_COUNTDOWN, Weight::Bold, Tokens::text_3(), icon_px::SM))
                            .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(format!("New code in {}", countdown(left))))
                            .child(
                                btn("new-code", "New code", ButtonKind::Ghost)
                                    .small()
                                    .icon(glyphs::REFRESH)
                                    .tooltip("Show a new code now; the current one stops working")
                                    .on_click(cx.listener(|this, _, _, cx| this.new_code(cx))),
                            ),
                    )
            }
            _ => {
                let msg = if self.status.running { "Preparing a code…" } else { "Starting phone access…" };
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(space::MD))
                    .size(px(QR_PX + 2. * space::MD))
                    .rounded(px(radius::LG))
                    .bg(Tokens::well())
                    .border_1()
                    .border_color(Tokens::border())
                    .child(spinner("qr-wait", Tokens::accent(), icon_px::XL))
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(msg))
            }
        };
        card()
            .flex_none()
            .w(px(QR_PX + 2. * space::MD + 2. * space::XXL))
            .p(px(space::XXL))
            .items_center()
            .gap(px(space::LG))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(ph_icon(ph::QR_CODE, Weight::Bold, Tokens::accent(), icon_px::MD))
                    .child(div().ramp(ramp::SUBTITLE_2).text_color(Tokens::text_1()).child("Scan to connect")),
            )
            .child(body)
            .child(
                div()
                    .text_center()
                    .text_size(px(size::SM))
                    .line_height(px(18.))
                    .text_color(Tokens::text_3())
                    .child("Scan with your phone's camera. The code works once and only for 5 minutes, so nobody else on your network can connect without it."),
            )
    }

    /// Status, the address and the steps beside the code.
    fn details(&self) -> Div {
        let addr = self.status.lan_ip.zip(self.status.addr).map(|(ip, a)| format!("http://{ip}:{}", a.port()));
        let status = if self.status.running {
            div()
                .flex()
                .items_center()
                .gap(px(space::SM))
                .child(pulse_dot("phone-live", Tokens::success(), 8.))
                .child(div().ramp(ramp::BODY_STRONG).text_color(Tokens::text_1()).child("Phone access is on"))
                .when_some(addr.clone(), |d, a| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(format!("Running at {a}"))))
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(space::SM))
                .child(spinner("phone-starting", Tokens::accent(), icon_px::SM))
                .child(div().ramp(ramp::BODY_STRONG).text_color(Tokens::text_1()).child("Starting phone access…"))
        };

        let url_row = addr.map(|a| {
            let copy = a.clone();
            div()
                .flex()
                .items_center()
                .gap(px(space::SM))
                .pl(px(space::MD))
                .pr(px(4.))
                .h(px(40.))
                .rounded(px(radius::MD))
                .bg(Tokens::well())
                .border_1()
                .border_color(Tokens::border())
                .child(ph_icon(ph::GLOBE_SIMPLE, Weight::Bold, Tokens::text_3(), icon_px::SM))
                .child(div().flex_1().min_w_0().truncate().font_family(MONO_FONT).text_size(px(size::SM)).text_color(Tokens::text_1()).child(a))
                .child(btn("copy-url", "Copy", ButtonKind::Secondary).small().icon(glyphs::COPY).tooltip("Copy the address").on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                    toast::show(cx, Toast::success("Address copied"));
                }))
        });

        let step = |n: usize, title: &'static str, body: &'static str| {
            div()
                .flex()
                .items_start()
                .gap(px(space::MD))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(26.))
                        .rounded(px(radius::PILL))
                        .bg(Tokens::accent_soft())
                        .border_1()
                        .border_color(Tokens::accent_border())
                        .text_size(px(size::SM))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(Tokens::accent())
                        .child(n.to_string()),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .pt(px(3.))
                        .child(div().ramp(ramp::BODY_STRONG).text_color(Tokens::text_1()).child(title))
                        .child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_3()).child(body)),
                )
        };

        card()
            .flex_1()
            .min_w(px(320.))
            .p(px(space::XXL))
            .gap(px(space::XL))
            .child(div().flex().flex_col().gap(px(space::MD)).child(status).children(url_row))
            .child(div().h(px(1.)).bg(Tokens::border()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::LG))
                    .child(step(1, "Join the same Wi-Fi", "Connect your phone to the network this PC is on."))
                    .child(step(2, "Scan the code", "Open the camera and point it at the code. Tap the link that appears."))
                    .child(step(3, "Add to Home screen", "In your browser's share or menu button, choose Add to Home screen for an app-like experience.")),
            )
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(space::SM))
                    .p(px(space::MD))
                    .rounded(px(radius::MD))
                    .bg(Tokens::surface_2())
                    .child(div().pt(px(2.)).child(ph_icon(ph::SHIELD_CHECK, Weight::Bold, Tokens::text_3(), icon_px::SM)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(size::SM))
                            .line_height(px(18.))
                            .text_color(Tokens::text_3())
                            .child("If your phone can't connect, allow MangaVox on Private networks when Windows asks, and make sure both devices are on the same Wi-Fi (not a guest network)."),
                    ),
            )
    }

    /// Connection check: the firewall rule, the network type and rules that would still block.
    fn connection(&self, cx: &mut Context<Self>) -> Div {
        let head = div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .px(px(space::LG))
            .py(px(space::MD))
            .child(ph_icon(ph::SHIELD_CHECK, Weight::Bold, Tokens::accent(), icon_px::MD))
            .child(div().flex_1().ramp(ramp::SUBTITLE_2).text_color(Tokens::text_1()).child("Connection check"))
            .child(
                btn("recheck", "Check again", ButtonKind::Ghost)
                    .small()
                    .icon(glyphs::REFRESH)
                    .loading(self.checking)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.run_checks(cx);
                        cx.notify();
                    })),
            );
        let Some(c) = &self.checks else {
            return card().child(head).child(
                check_row(Check::Busy, "Checking Windows Firewall and your network…", "This takes a second.", None).border_t_1().border_color(Tokens::border()),
            );
        };

        let firewall = if c.rule {
            check_row(Check::Ok, "Windows Firewall", "Phones on Private networks are allowed to connect.", None)
        } else {
            let hint = match &self.fw_error {
                Some(e) => format!("Not allowed yet. {e}"),
                None => "Windows may block phones until MangaVox is allowed. Windows asks for permission once.".into(),
            };
            check_row(
                Check::Warn,
                "Windows Firewall",
                hint,
                Some(
                    btn("allow-fw", "Allow phones through Windows Firewall", ButtonKind::Primary)
                        .small()
                        .icon(ph::SHIELD_CHECK)
                        .loading(self.fw_busy)
                        .on_click(cx.listener(|this, _, _, cx| this.allow_firewall(cx)))
                        .into_any_element(),
                ),
            )
        };

        let network = match c.private {
            Some(true) => check_row(Check::Ok, "Network type", "This Wi-Fi is set to Private.", None),
            Some(false) => check_row(
                Check::Warn,
                "Network type",
                "Your Wi-Fi is set to Public, so Windows blocks phones. Set it to Private in the network's properties.",
                Some(
                    btn("net-settings", "Open network settings", ButtonKind::Secondary)
                        .small()
                        .trailing_icon(glyphs::EXTERNAL)
                        .on_click(cx.listener(|this, _, _, cx| this.open_network_settings(cx)))
                        .into_any_element(),
                ),
            ),
            None => check_row(Check::Info, "Network type", "Windows couldn't tell whether this network is Private. Phones connect only on Private networks.", None),
        };

        let blocking = (!c.blocking.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(space::SM))
                .child(check_row(
                    Check::Warn,
                    "Blocking firewall rules",
                    "These firewall rules block incoming connections; remove or disable them in Windows Defender Firewall.",
                    None,
                ))
                .child(
                    div().flex().flex_col().gap(px(4.)).pl(px(space::LG + 28.)).pr(px(space::LG)).pb(px(space::MD)).children(c.blocking.iter().map(|name| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::SM))
                            .child(ph_icon(ph::PROHIBIT, Weight::Bold, Tokens::warning(), icon_px::XS))
                            .child(div().min_w_0().truncate().font_family(MONO_FONT).text_size(px(size::SM)).text_color(Tokens::text_2()).child(name.clone()))
                    })),
                )
        });

        card()
            .child(head)
            .child(firewall.border_t_1().border_color(Tokens::border()))
            .child(network.border_t_1().border_color(Tokens::border()))
            .when_some(blocking, |d, b| d.child(b.border_t_1().border_color(Tokens::border())))
    }

    fn phones(&self, cx: &mut Context<Self>) -> Div {
        let n = self.devices.len();
        let now = unix_now();
        let mut head = section_header("Paired phones", Some(ph::DEVICE_MOBILE)).count(n);
        if n > 0 && !self.confirm_remove_all {
            head = head.trailing(btn("remove-all", "Remove all", ButtonKind::DangerGhost).small().icon(glyphs::DELETE).on_click(cx.listener(|this, _, _, cx| {
                this.confirm_remove_all = true;
                cx.notify();
            })));
        }
        let body = if n == 0 {
            div().child(empty_view(ph::DEVICE_MOBILE, "No phones paired yet", "Phones you pair by scanning the code show up here.").compact())
        } else {
            let rows = self.devices.iter().enumerate().map(|(i, d)| {
                let (id, name) = (d.id.clone(), d.name.clone());
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::MD))
                    .px(px(space::LG))
                    .py(px(space::MD))
                    .when(i > 0, |r| r.border_t_1().border_color(Tokens::border()))
                    .child(tile(ph::DEVICE_MOBILE, 36.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(div().ramp(ramp::BODY_STRONG).text_color(Tokens::text_1()).truncate().child(d.name.clone()))
                            .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).truncate().child(format!("Last active {} · Paired {}", ago(now - d.last_seen), ago(now - d.created)))),
                    )
                    .child(
                        btn(("remove", i), "Remove", ButtonKind::DangerGhost)
                            .small()
                            .tooltip("Sign this phone out; it needs a new code to come back")
                            .on_click(cx.listener(move |this, _, _, cx| this.remove(id.clone(), name.clone(), cx))),
                    )
            });
            card().children(rows)
        };
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .child(head)
            .when(self.confirm_remove_all, |d| {
                d.child(
                    confirm_inline("Remove every paired phone? Each one needs a new code to connect again.", "Remove all phones", true)
                        .on_confirm(cx.listener(|this, _, _, cx| this.remove_all(cx)))
                        .on_cancel(cx.listener(|this, _, _, cx| {
                            this.confirm_remove_all = false;
                            cx.notify();
                        })),
                )
            })
            .child(body)
    }
}

impl Render for MobilePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.enabled;
        let header = mv_ui::page_title("Mobile").icon(ph::DEVICE_MOBILE).subtitle(SUBTITLE).action(
            switch("phone-access", on)
                .label("Phone access")
                .tooltip(if on { "Turn off: phones disconnect, pairings are kept" } else { "Let paired phones on this Wi-Fi connect" })
                .on_click(cx.listener(|this, on: &bool, _, cx| this.set_enabled(*on, cx))),
        );

        let main = if !on {
            self.intro(cx).into_any_element()
        } else if let Some(err) = self.error() {
            div()
                .flex()
                .flex_col()
                .gap(px(space::LG))
                .child(inline_alert(AlertKind::Error, "Phone access could not start", err).action("Retry", cx.listener(|this, _, _, cx| this.retry(cx))))
                .child(
                    div()
                        .text_size(px(size::SM))
                        .text_color(Tokens::text_3())
                        .child("If another app uses the port, MangaVox tries the next free one. If Windows Firewall asked, allow MangaVox on Private networks."),
                )
                .into_any_element()
        } else {
            div().flex().flex_wrap().items_start().gap(px(space::XL)).child(self.qr_card(cx)).child(self.details()).into_any_element()
        };

        mv_ui::page("mobile-page").child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .max_w(px(1040.))
                .gap(px(space::XXL))
                .child(header)
                .child(main)
                .when(on, |d| d.child(self.connection(cx)))
                .child(self.phones(cx)),
        )
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Check {
    Ok,
    Warn,
    Info,
    Busy,
}

/// One line of the connection check: status icon, title, hint and an optional fix button.
fn check_row(state: Check, title: &'static str, hint: impl Into<SharedString>, fix: Option<AnyElement>) -> Div {
    let mark = match state {
        Check::Ok => ph_icon(ph::CHECK_CIRCLE, Weight::Fill, Tokens::success(), icon_px::LG).into_any_element(),
        Check::Warn => ph_icon(ph::WARNING, Weight::Fill, Tokens::warning(), icon_px::LG).into_any_element(),
        Check::Info => ph_icon(ph::INFO, Weight::Fill, Tokens::info(), icon_px::LG).into_any_element(),
        Check::Busy => spinner("check-busy", Tokens::accent(), icon_px::LG).into_any_element(),
    };
    div()
        .flex()
        .items_center()
        .gap(px(space::MD))
        .px(px(space::LG))
        .py(px(space::MD))
        .child(div().flex_none().size(px(icon_px::LG)).child(mark))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(div().ramp(ramp::BODY_STRONG).text_color(Tokens::text_1()).child(title))
                .child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_3()).child(hint.into())),
        )
        .children(fix)
}

/// A card: raised surface, hairline border, large radius.
fn card() -> Div {
    div().flex().flex_col().rounded(px(radius::XL)).bg(Tokens::surface()).border_1().border_color(Tokens::border())
}

/// An accent icon in a soft square tile.
fn tile(icon: Ph, side: f32) -> Div {
    let glyph = if side >= 48. { icon_px::HERO } else { icon_px::LG };
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(side))
        .rounded(px(if side >= 48. { radius::LG } else { radius::MD }))
        .bg(Tokens::accent_soft())
        .child(ph_icon(icon, Weight::Duotone, Tokens::accent(), glyph))
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// "4:12".
fn countdown(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// "just now", "2 min ago", "3 h ago", "5 days ago".
fn ago(secs: i64) -> String {
    match secs.max(0) {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} h ago", s / 3600),
        s if s < 2 * 86_400 => "yesterday".into(),
        s => format!("{} days ago", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::{ago, countdown};

    #[test]
    fn times_read_naturally() {
        assert_eq!(countdown(252), "4:12");
        assert_eq!(countdown(5), "0:05");
        assert_eq!(ago(10), "just now");
        assert_eq!(ago(150), "2 min ago");
        assert_eq!(ago(7200), "2 h ago");
        assert_eq!(ago(90_000), "yesterday");
        assert_eq!(ago(5 * 86_400), "5 days ago");
        assert_eq!(ago(-3), "just now");
    }
}

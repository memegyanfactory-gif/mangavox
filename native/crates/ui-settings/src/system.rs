//! Settings → System: the resource limit for heavy work (`mv_core::limits`) and a live
//! meter of CPU and memory use, for the whole PC and for MangaVox (app + its workers).
use std::collections::HashSet;
use std::sync::Mutex;

use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::*;
use mv_core::limits;
use mv_ui::{size, Tokens};
use sysinfo::{Pid, ProcessesToUpdate, System};

use crate::ui::{card, divided, format_bytes, row};
use crate::SettingsView;

/// One reading of the meter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meter {
    /// 0–100, whole PC.
    pub cpu: f32,
    /// 0–100 of all CPUs, MangaVox and every process it started.
    pub app_cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub app_mem: u64,
    /// Processes running under the limit right now.
    pub capped: u32,
}

static SYS: Mutex<Option<System>> = Mutex::new(None);

/// Samples CPU and memory. CPU figures are averages since the previous call, so call it
/// about once a second. Blocking; run off the UI thread.
pub fn sample() -> Meter {
    let mut guard = SYS.lock().unwrap_or_else(|e| e.into_inner());
    let sys = guard.get_or_insert_with(System::new);
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    sys.refresh_processes(ProcessesToUpdate::All);
    let me = Pid::from_u32(std::process::id());
    let tree = descendants(sys, me);
    let cores = sys.cpus().len().max(1) as f32;
    let (mut app_cpu, mut app_mem) = (0.0f32, 0u64);
    for pid in &tree {
        if let Some(p) = sys.process(*pid) {
            app_cpu += p.cpu_usage();
            app_mem += p.memory();
        }
    }
    Meter {
        cpu: sys.global_cpu_usage().clamp(0.0, 100.0),
        app_cpu: (app_cpu / cores).clamp(0.0, 100.0),
        mem_used: sys.used_memory(),
        mem_total: sys.total_memory(),
        app_mem,
        capped: limits::heavy_processes(),
    }
}

/// `root` and every process below it.
fn descendants(sys: &System, root: Pid) -> HashSet<Pid> {
    let mut set = HashSet::from([root]);
    // Parents can appear after children in the map, so repeat until nothing is added.
    loop {
        let before = set.len();
        for (pid, p) in sys.processes() {
            if p.parent().is_some_and(|parent| set.contains(&parent)) {
                set.insert(*pid);
            }
        }
        if set.len() == before {
            return set;
        }
    }
}

/// Slider for the limit, wired to save and apply on release.
pub fn limit_slider(window: &mut Window, cx: &mut Context<SettingsView>) -> Entity<SliderState> {
    let state = cx.new(|_| {
        SliderState::new()
            .min(limits::MIN_PERCENT as f32)
            .max(limits::MAX_PERCENT as f32)
            .step(5.)
            .default_value(limits::percent() as f32)
    });
    cx.subscribe_in(&state, window, |this: &mut SettingsView, _, ev: &SliderEvent, _, cx| match ev {
        SliderEvent::Change(v) => {
            this.limit = limits::clamp(v.start().round() as i64);
            cx.notify();
        }
        SliderEvent::Release(v) => {
            let p = limits::clamp(v.start().round() as i64);
            this.limit = p;
            this.background(cx, move || limits::set_percent(p), move |v, r, cx| v.report(&format!("Heavy work is now limited to {p}% of this PC"), r, cx));
        }
    })
    .detach();
    state
}

fn bar(fraction: f32, color: Rgba) -> Div {
    div()
        .w(px(220.))
        .h(px(6.))
        .rounded(px(3.))
        .bg(Tokens::surface_2())
        .child(div().h_full().rounded(px(3.)).bg(color).w(relative(fraction.clamp(0.0, 1.0))))
}

fn meter_row(label: &str, hint: &str, fraction: f32, value: String, limit: Option<f32>) -> Div {
    let color = if limit.is_some_and(|l| fraction > l + 0.02) { Tokens::warning() } else { Tokens::accent() };
    row(
        label,
        hint,
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(bar(fraction, color))
            .child(div().w(px(130.)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(value)),
    )
}

impl SettingsView {
    pub(crate) fn render_system(&self, _cx: &mut Context<Self>) -> Div {
        let p = self.limit;
        let cores = limits::cores();
        let threads = (cores * p as usize / 100).max(1);
        let ram = self.meter.as_ref().map(|m| m.mem_total).unwrap_or(0);
        let detail = if p >= limits::MAX_PERCENT {
            "No limit: heavy work may use the whole PC. It still runs at below-normal priority so the mouse stays smooth.".to_string()
        } else {
            format!(
                "Heavy work may use at most {p}% of the CPU (about {threads} of {cores} threads){}, at below-normal priority. Changes apply at once, also to work already running.",
                if ram > 0 { format!(" and {} of memory", format_bytes(ram / 100 * p as u64)) } else { String::new() }
            )
        };
        let limit = card(
            "Resource limit",
            Some("How much of this PC MangaVox's heavy work may use: voice narration, video rendering, Python workers and local models. Lower it if the PC feels slow while Studio works."),
        )
        .child(row(
            "Limit",
            &detail,
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(div().w(px(200.)).child(Slider::new(&self.limit_slider)))
                .child(div().w(px(44.)).text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("{p}%"))),
        ));

        let now = card("Right now", Some("Updated every second."));
        let now = match &self.meter {
            None => now.child(row("Measuring…", "", div())),
            Some(m) => {
                let mem = |b: u64| if m.mem_total > 0 { b as f32 / m.mem_total as f32 } else { 0.0 };
                let cap = (p < limits::MAX_PERCENT).then_some(p as f32 / 100.0);
                now.child(meter_row("CPU, whole PC", "Everything running on this computer.", m.cpu / 100.0, format!("{:.0}%", m.cpu), None))
                    .child(divided(meter_row(
                        "CPU, MangaVox",
                        &if m.capped > 0 { format!("App plus {} worker process{} under the limit.", m.capped, if m.capped == 1 { "" } else { "es" }) } else { "App plus any workers it starts.".into() },
                        m.app_cpu / 100.0,
                        format!("{:.0}%", m.app_cpu),
                        cap,
                    )))
                    .child(divided(meter_row("Memory, whole PC", "", mem(m.mem_used), format!("{} of {}", format_bytes(m.mem_used), format_bytes(m.mem_total)), None)))
                    .child(divided(meter_row("Memory, MangaVox", "", mem(m.app_mem), format_bytes(m.app_mem), cap)))
            }
        };
        div().flex().flex_col().gap(px(16.)).child(limit).child(now)
    }

    /// Takes a meter reading off the UI thread (one at a time).
    pub(crate) fn sample_meter(&mut self, cx: &mut Context<Self>) {
        if self.sampling {
            return;
        }
        self.sampling = true;
        cx.spawn(async move |this, cx| {
            let m = mv_ui::bg::blocking(sample).await.ok();
            this.update(cx, |v, cx| {
                v.sampling = false;
                if m.is_some() {
                    v.meter = m;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn meter_sees_this_process() {
        let _ = super::sample();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let m = super::sample();
        assert!(m.mem_total > 0 && m.mem_used > 0 && m.mem_used <= m.mem_total);
        assert!(m.app_mem > 0, "own process memory should be counted");
        assert!((0.0..=100.0).contains(&m.cpu) && (0.0..=100.0).contains(&m.app_cpu));
    }
}

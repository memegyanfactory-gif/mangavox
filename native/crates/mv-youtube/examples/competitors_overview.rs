//! Prints the Competitors overview of a competitors folder (`<data>/youtube/competitors`):
//! `cargo run -p mv-youtube --example competitors_overview -- <folder>`.
use mv_youtube::competitors::{overview, CompStore};

fn main() {
    let dir = std::env::args().nth(1).expect("the competitors folder");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let started = std::time::Instant::now();
    let o = overview(&CompStore::new(dir), now, 0);
    println!("{} channels, {} videos ({} read in full), {} compared, {} winners, in {:.2}s", o.channels, o.videos, o.read_in_full, o.compared, o.winners, started.elapsed().as_secs_f64());
    for t in &o.top {
        println!("  top  {:>9.0}/day  {:>6}  {:.60}  [{}]", t.views_per_day, t.lift.map(|l| format!("x{l:.1}")).unwrap_or_default(), t.video.title, t.channel);
    }
    for b in &o.lengths {
        println!("  len  {:<13} {:>4} videos  x{:.2}  {:.0}% hits  median {} views", b.label, b.videos, b.lift, b.hit_rate * 100.0, b.median_views);
    }
    println!("  hours {:?}\n  days  {:?} ({} timed)", o.upload_hours, o.upload_days, o.timed);
    println!("  title chars: winners {}, all {}", o.winner_title_chars, o.title_chars);
    println!("  words {}", o.words.iter().map(|w| format!("{} x{:.1} ({})", w.word, w.lift, w.winners)).collect::<Vec<_>>().join(", "));
    println!("  tags  {}", o.tags.iter().map(|(t, n)| format!("{t} ({n})")).collect::<Vec<_>>().join(", "));
    for m in &o.movers {
        println!("  mover {:<30} +{}", m.title, m.gained);
    }
    for c in &o.changes {
        println!("  change [{}] {:.50} -> {:.50}", c.channel, c.before, c.after);
    }
}

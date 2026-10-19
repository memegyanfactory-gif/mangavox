//! Scores a chapter folder (`<dir>/audio/page_NNNN.wav` + `<dir>/studio/read.json`) and prints the cue
//! sheet: `cargo run -p mv-score --example score_chapter -- <dir>`. Writes into `<dir>`, so point it at a copy.

fn main() -> anyhow::Result<()> {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("chapter dir"));
    let ch = mv_recap::ChapterRef { chapter_id: "demo".into(), label: "demo".into(), pages_dir: dir.join("pages"), output_dir: dir.clone(), ..Default::default() };
    let started = std::time::Instant::now();
    let (score, report) = mv_score::build(&ch, &mv_score::load_settings(&ch), true)?;
    for c in &score.cues {
        println!(
            "{:7.1}-{:7.1}s  p{:>3}-{:<3} {}  energy {:.1}{}{}{}",
            c.start,
            c.end,
            c.pages.0 + 1,
            c.pages.1 + 1,
            c.label(),
            c.energy,
            if c.hit { " HIT" } else { "" },
            if c.drop_before { " DROP" } else { "" },
            if c.riser_out { " RISER" } else { "" }
        );
    }
    let sfx = score.track(mv_score::Role::Sfx).map(|t| t.clips.iter().map(|c| c.notes.len()).sum::<usize>()).unwrap_or(0);
    // Loudness of each track on its own (before the mixer strip and master).
    let orch = mv_score::orchestra::load();
    for t in &score.tracks {
        let r = mv_score::mix::render_track(t, (score.duration * 44_100.0) as usize, &orch, 1);
        let notes: usize = t.clips.iter().map(|c| c.notes.len()).sum();
        println!("  {:<16} {:>6} notes  {:>6.1} LUFS", t.role.label(), notes, mv_score::mix::loudness(&r.l, &r.r, 44_100).unwrap_or(-99.0));
    }
    // The music alone, not ducked, for listening to the score itself.
    let mut open = mv_score::load_settings(&ch);
    open.duck_db = 0.0;
    let (bus, _, _) = mv_score::mix::render(&score, &open, &Default::default(), &orch, 7, false);
    mv_score::mix::write_wav(&dir.join("music_only.wav"), &bus)?;
    println!("{} cues, {sfx} sound effects, orchestra {}, built in {:.1}s: {report:?}", score.cues.len(), score.orchestra, started.elapsed().as_secs_f64());
    Ok(())
}

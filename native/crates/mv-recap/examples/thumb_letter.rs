//! Letters labels on a picture, as the Thumbnails page does after the art is made:
//! `cargo run -p mv-recap --example thumb_letter -- <art> <out.png> <left|right> "<label>" ["<tag>" ["<tag>"]]`
//! (the default places), or `... <art> <out.png> <lettering.json>` (places given, in a 1280x720 frame).
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(a.len() >= 3, "usage: thumb_letter <art> <out.png> (<left|right> <label> [tag] [tag] | <lettering.json>)");
    let l = if a[2].ends_with(".json") {
        serde_json::from_slice(&std::fs::read(&a[2])?)?
    } else {
        anyhow::ensure!(a.len() >= 4, "a label is needed");
        mv_recap::thumb::default_lettering(&a[3], "", &a[4..], a[2] != "right")
    };
    mv_recap::thumb::letter(a[0].as_ref(), &l, a[1].as_ref())
}

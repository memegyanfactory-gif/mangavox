//! Live streaming benchmark (network!). Measures what the player feels:
//! metadata time, first byte, seek latency at 0/50/90/10 %, throughput.
//!
//! ```text
//! cargo run -p mv-torrent --release --example bench -- [--prefetch] [--dir D] [--session S] [--file N] [-v] <magnet>
//! ```
//! Without a magnet it uses Sintel (public domain). A fresh temp cache dir is
//! used unless `--dir` is given (pass the same dir twice to measure a warm
//! cache). `--session S` keeps DHT / fastresume state in S between runs (what
//! the app does) while still starting with an empty data cache.

use std::time::{Duration, Instant};

use futures::StreamExt;
use mv_torrent::{AddOptions, Engine, EngineConfig, Source};

const SINTEL: &str = "magnet:?xt=urn:btih:08ada5a7a6183aae1e09d831df6748d566095a10&dn=Sintel&tr=udp%3A%2F%2Fexplodie.org%3A6969&tr=udp%3A%2F%2Ftracker.coppersurfer.tk%3A6969&tr=udp%3A%2F%2Ftracker.empire-js.us%3A1337&tr=udp%3A%2F%2Ftracker.leechers-paradise.org%3A6969&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337&tr=wss%3A%2F%2Ftracker.btorrent.xyz&tr=wss%3A%2F%2Ftracker.fastcast.nz&tr=wss%3A%2F%2Ftracker.openwebtorrent.com&ws=https%3A%2F%2Fwebtorrent.io%2Ftorrents%2F";

fn ms(d: Duration) -> String {
    format!("{:>7.0} ms", d.as_secs_f64() * 1000.0)
}

/// mpv-style open-ended range read: returns (headers, first byte, `want` bytes).
async fn range_read(c: &reqwest::Client, url: &str, start: u64, want: usize) -> anyhow::Result<(Duration, Duration, Duration)> {
    let t = Instant::now();
    let r = c.get(url).header("Range", format!("bytes={start}-")).send().await?;
    anyhow::ensure!(r.status() == 206, "status {}", r.status());
    let headers = t.elapsed();
    let mut body = r.bytes_stream();
    let mut got = 0usize;
    let mut first = None;
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        if first.is_none() && !chunk.is_empty() {
            first = Some(t.elapsed());
        }
        got += chunk.len();
        if got >= want {
            break;
        }
    }
    // Dropping `body` here closes the connection, like mpv does on seek.
    Ok((headers, first.unwrap_or_default(), t.elapsed()))
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut magnet, mut dir, mut file_idx, mut prefetch, mut verbose) = (None, None, None, false, false);
    let mut session = None;
    let mut peers: Option<usize> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dir" => dir = args.next(),
            "--session" => session = args.next(),
            "--peers" => peers = args.next().and_then(|s| s.parse().ok()),
            "--file" => file_idx = args.next().and_then(|s| s.parse().ok()),
            "--prefetch" => prefetch = true,
            "-v" | "--verbose" => verbose = true,
            _ => magnet = Some(a),
        }
    }
    let magnet = magnet.unwrap_or_else(|| SINTEL.to_string());
    let (dir, temp) = match dir {
        Some(d) => (std::path::PathBuf::from(d), false),
        None => (std::env::temp_dir().join(format!("mv-bench-{}", std::process::id())), true),
    };
    println!("cache dir: {}", dir.display());

    let t0 = Instant::now();
    let mut cfg = EngineConfig::new(&dir);
    cfg.session_dir = session.map(Into::into);
    if peers.is_some() {
        cfg.peer_limit = peers;
    }
    let engine = Engine::new(cfg).await?;
    println!("engine start      {}", ms(t0.elapsed()));

    let source = Source::parse(&magnet)?;
    let opts = AddOptions { file_idx, ..Default::default() };
    if prefetch {
        // Simulates the detail page warming the source before the user hits play.
        let t = Instant::now();
        engine.prefetch(source.clone(), opts.clone())?;
        tokio::time::sleep(Duration::from_secs(8)).await;
        println!("prefetch warm-up  {} (simulated detail-page dwell)", ms(t.elapsed()));
    }

    let t = Instant::now();
    let added = engine.add(source, opts).await?;
    println!(
        "metadata+init     {}   file #{} {} ({:.1} MiB)",
        ms(t.elapsed()),
        added.file_idx,
        added.file_name,
        added.file_len as f64 / 1048576.0
    );

    let engine = std::sync::Arc::new(engine);
    if verbose {
        let (e, h) = (engine.clone(), added.info_hash.clone());
        tokio::spawn(async move {
            let t = Instant::now();
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                if let Ok(s) = e.stats(&h) {
                    println!(
                        "  [{:>4.0}s] peers {:>3}  {:>6.2} MB/s  progress {:>5.1}%",
                        t.elapsed().as_secs_f64(),
                        s.peers,
                        s.download_speed / 1e6,
                        s.progress * 100.0
                    );
                }
            }
        });
    }

    let c = reqwest::Client::builder().pool_max_idle_per_host(0).build()?;
    let url = added.stream_url.clone();
    let len = added.file_len;

    // HEAD like a probing demuxer.
    let t = Instant::now();
    let h = c.head(&url).send().await?;
    println!("HEAD              {}   status {}", ms(t.elapsed()), h.status());

    const WANT: usize = 1 << 20;
    let points = [("0%", 0u64), ("tail-64K", len.saturating_sub(65536)), ("50%", len / 2), ("90%", len / 10 * 9), ("10%", len / 10)];
    for (name, off) in points {
        let want = WANT.min((len - off) as usize);
        match range_read(&c, &url, off, want).await {
            Ok((hd, first, all)) => println!(
                "seek {name:<9} hdr {}  first byte {}  1MiB {}",
                ms(hd),
                ms(first),
                ms(all)
            ),
            Err(e) => println!("seek {name:<9} FAILED: {e:#}"),
        }
    }

    // Sustained sequential throughput from 10 % (what playback needs).
    let t = Instant::now();
    let r = c.get(&url).header("Range", format!("bytes={}-", len / 10)).send().await?;
    let mut body = r.bytes_stream();
    let mut got = 0u64;
    while let Ok(Some(chunk)) = tokio::time::timeout(Duration::from_secs(15).saturating_sub(t.elapsed()), body.next()).await {
        got += chunk?.len() as u64;
        if t.elapsed() >= Duration::from_secs(15) {
            break;
        }
    }
    drop(body);
    let secs = t.elapsed().as_secs_f64();
    let st = engine.stats(&added.info_hash)?;
    println!(
        "sequential        {:.2} MB/s over {:.0}s ({:.1} MiB)  peers {}  swarm {:.2} MB/s  progress {:.1}%",
        got as f64 / secs / 1e6,
        secs,
        got as f64 / 1048576.0,
        st.peers,
        st.download_speed / 1e6,
        st.progress * 100.0
    );
    println!("total             {}", ms(t0.elapsed()));

    if temp {
        let _ = engine.remove(&added.info_hash, true).await;
    }
    if let Ok(engine) = std::sync::Arc::try_unwrap(engine) {
        engine.shutdown().await;
    }
    if temp {
        let _ = std::fs::remove_dir_all(&dir);
    }
    Ok(())
}

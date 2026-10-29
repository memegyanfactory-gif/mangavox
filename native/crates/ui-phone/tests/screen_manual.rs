//! Manual check of the remote screen against a running MangaVox dev build (never the installed app).
//! Start `target\debug\mangavox.exe`, then:
//!   cargo test -p ui-phone --test screen_manual -- --ignored --nocapture
//! It captures the window to %TEMP%\mv-screen-test.jpg, posts `MV_SCREEN_INPUT` (a JSON gesture,
//! default: scroll down 3 notches at the center), and captures again to %TEMP%\mv-screen-test-2.jpg.
//! Only windows of a process whose exe lives under `MV_SCREEN_EXE_DIR` (default D:\mv-remote) are used.
#![cfg(windows)]

use ui_phone::screen::{self, Input};

fn dev_window() -> Option<isize> {
    let dir = std::env::var("MV_SCREEN_EXE_DIR").unwrap_or_else(|_| r"D:\mv-remote".into()).to_ascii_lowercase();
    let only: Option<u32> = std::env::var("MV_SCREEN_PID").ok().and_then(|p| p.parse().ok());
    screen::find_window(|pid, title| {
        title.ends_with("MangaVox")
            && only.map_or(true, |o| o == pid)
            && screen::process_path(pid).is_some_and(|p| p.to_ascii_lowercase().starts_with(&dir))
    })
}

#[test]
#[ignore]
fn capture_and_input() {
    let hwnd = dev_window().expect("no MangaVox dev window running");
    let tmp = std::env::temp_dir();
    let t = std::time::Instant::now();
    let (jpeg, size) = screen::capture_hwnd(hwnd, 900, 60).expect("capture");
    println!("capture 1: {} bytes, client {size:?}, method {}, {:?}", jpeg.len(), screen::last_method(), t.elapsed());
    std::fs::write(tmp.join("mv-screen-test.jpg"), &jpeg).unwrap();

    let cursor = || {
        let mut p = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p) };
        (p.x, p.y)
    };
    let before = cursor();
    let gesture = std::env::var("MV_SCREEN_INPUT").unwrap_or_else(|_| r#"{"t":"scroll","x":0.6,"y":0.6,"dy":3}"#.into());
    for g in gesture.split(";;") {
        let input: Input = serde_json::from_str(g).expect("MV_SCREEN_INPUT");
        println!("posting {input:?}");
        screen::post_input(hwnd, &input).expect("post");
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    std::thread::sleep(std::time::Duration::from_millis(800));
    println!("real cursor before {before:?} after {:?}", cursor());

    let t = std::time::Instant::now();
    let (jpeg, size) = screen::capture_hwnd(hwnd, 1600, 80).expect("capture");
    println!("capture 2: {} bytes, client {size:?}, method {}, {:?}", jpeg.len(), screen::last_method(), t.elapsed());
    std::fs::write(tmp.join("mv-screen-test-2.jpg"), &jpeg).unwrap();
}

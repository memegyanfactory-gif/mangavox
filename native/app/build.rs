//! Embeds the app icon as icon resource 1, which GPUI loads for the window,
//! taskbar and Alt+Tab (and Explorer shows for the exe).
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon_with_id("assets/icon.ico", "1");
        res.set("ProductName", "MangaVox").set("FileDescription", "MangaVox");
        res.compile().expect("embed Windows resources");
    }
}

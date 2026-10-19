//! Embeds the app payload (`MANGAVOX_PAYLOAD` = path to MangaVox-<ver>-win64.zip, set by
//! scripts/package.ps1). Without it the setup downloads the latest release instead.
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=MANGAVOX_PAYLOAD");
    println!("cargo:rerun-if-changed=icon.ico");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("payload.zip");
    match std::env::var_os("MANGAVOX_PAYLOAD").map(PathBuf::from).filter(|p| p.is_file()) {
        Some(p) => {
            println!("cargo:rerun-if-changed={}", p.display());
            std::fs::copy(&p, &out).expect("copy payload");
        }
        None => std::fs::write(&out, b"").expect("write empty payload"),
    }
    #[cfg(windows)]
    {
        let v = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
        let mut res = winresource::WindowsResource::new();
        res.set_icon_with_id("icon.ico", "1");
        res.set("ProductName", "MangaVox Setup")
            .set("FileDescription", "MangaVox Setup")
            .set("CompanyName", "Aayush Datta")
            .set("LegalCopyright", "Copyright Aayush Datta")
            .set("ProductVersion", &v)
            .set("FileVersion", &v);
        res.compile().expect("embed Windows resources");
    }
}

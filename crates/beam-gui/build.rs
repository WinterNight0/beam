//! Puts `assets/images/beam-gui.ico` inside the Windows .exe as its icon, for
//! Explorer, the Start menu and pinned taskbar buttons (ADR-0055). Other
//! systems take a program's icon from a menu entry instead:
//! `packaging/linux/`.

fn main() {
    println!("cargo:rerun-if-changed=assets/images/beam-gui.ico");
    println!("cargo:rerun-if-changed=assets/images/beam-gui.rc");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/images/beam-gui.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("the resource compiler could not build assets/images/beam-gui.rc");
    }
}

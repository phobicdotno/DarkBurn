// Windows: embeds the app icon as resource 1 (the taskbar and Explorer use it; native_frame.rs
// gives it to the window class) and a version resource. Pattern from SolidCraft's app build.rs.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icons/app/darkburn.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS")? != "windows" {
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("missing OUT_DIR")?);
        let copy = out.join("darkburn.ico");
        std::fs::copy(icon, &copy)?;
        let version = std::env::var("CARGO_PKG_VERSION")?;
        let mut numbers = version.split(['-', '+']).next().unwrap_or("").split('.').map(|p| p.parse::<u16>().unwrap_or(0));
        let mut next = || numbers.next().unwrap_or(0);
        let (major, minor, patch) = (next(), next(), next());
        let rc = out.join("darkburn.rc");
        let text = format!(
            "1 ICON \"{icon}\"\n1 VERSIONINFO\nFILEVERSION {major},{minor},{patch},0\nPRODUCTVERSION {major},{minor},{patch},0\nFILEOS 0x40004\nFILETYPE 0x1\n{{\nBLOCK \"StringFileInfo\"\n{{\nBLOCK \"040904B0\"\n{{\nVALUE \"CompanyName\", \"DarkBurn contributors\"\nVALUE \"FileDescription\", \"DarkBurn laser cutter and engraver\"\nVALUE \"FileVersion\", \"{version}\"\nVALUE \"InternalName\", \"darkburn\"\nVALUE \"OriginalFilename\", \"darkburn.exe\"\nVALUE \"ProductName\", \"DarkBurn\"\nVALUE \"ProductVersion\", \"{version}\"\n}}\n}}\nBLOCK \"VarFileInfo\"\n{{\nVALUE \"Translation\", 0x0409, 1200\n}}\n}}\n",
            icon = copy.display().to_string().replace('\\', "/")
        );
        std::fs::write(&rc, text)?;
        embed_resource::compile(&rc, embed_resource::NONE).manifest_optional()?;
    }
    Ok(())
}

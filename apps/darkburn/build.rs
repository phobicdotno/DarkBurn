// Every platform: lists the translation files in assets/locales for src/languages.rs, so a new
// language is a new file there (and its line in the table of languages).
// Windows: embeds the app icon as resource 1 (the taskbar and Explorer use it; native_frame.rs
// gives it to the window class) and a version resource. Pattern from SolidCraft's app build.rs.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icons/app/darkburn.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed=build.rs");
    locale_files()?;
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

/// Writes `$OUT_DIR/locale_files.rs`: a `(code, text)` row with `include_str!` for every
/// `assets/locales/<code>.json`, sorted by code.
fn locale_files() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/locales");
    // A directory: Cargo looks at every file in it, so an added file is picked up.
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut codes = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(code) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        if code.is_empty() || !code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            println!("cargo:warning=assets/locales/{code}.json: a language code has letters, digits and '-' only; left out");
            continue;
        }
        codes.push(code.to_string());
    }
    codes.sort();
    let mut text =
        String::from("/// Every translation file in `assets/locales`, by code. Written by build.rs.\npub const LOCALE_FILES: &[(&str, &str)] = &[\n");
    for code in codes {
        text.push_str(&format!("    (\"{code}\", include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../../assets/locales/{code}.json\"))),\n"));
    }
    text.push_str("];\n");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("missing OUT_DIR")?);
    std::fs::write(out.join("locale_files.rs"), text)?;
    Ok(())
}

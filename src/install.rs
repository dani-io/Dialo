//! `dialo --install`: the binary to `~/.local/bin`, a launcher entry, and Dialo as the
//! handler of `tel:`, `sip:` and `callto:` links. `--uninstall` takes them out again
//! (settings, contacts and recent calls stay).

use std::path::PathBuf;

const DESKTOP: &str = include_str!("../packaging/dialo.desktop");
const SCHEMES: [&str; 4] = ["x-scheme-handler/tel", "x-scheme-handler/sip", "x-scheme-handler/sips", "x-scheme-handler/callto"];

fn places() -> Result<(PathBuf, PathBuf), String> {
    let home = dirs::home_dir().ok_or("no home folder")?;
    let bin = home.join(".local/bin/dialo");
    let apps = dirs::data_dir().unwrap_or_else(|| home.join(".local/share")).join("applications/dialo.desktop");
    Ok((bin, apps))
}

pub fn install() -> Result<(), String> {
    let (bin, desktop) = places()?;
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(bin.parent().unwrap()).map_err(|e| e.to_string())?;
    if me != bin {
        // Through a temporary name, so a running Dialo's binary is replaced, not rewritten.
        let tmp = bin.with_extension("new");
        std::fs::copy(&me, &tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &bin).map_err(|e| format!("{}: {e}", bin.display()))?;
    }
    std::fs::create_dir_all(desktop.parent().unwrap()).map_err(|e| e.to_string())?;
    let entry = DESKTOP.replace("Exec=dialo", &format!("Exec={}", bin.display()));
    std::fs::write(&desktop, entry).map_err(|e| format!("{}: {e}", desktop.display()))?;
    let _ = std::process::Command::new("update-desktop-database").arg(desktop.parent().unwrap()).status();
    let linked = std::process::Command::new("xdg-mime").arg("default").arg("dialo.desktop").args(SCHEMES).status().is_ok_and(|s| s.success());
    println!("Dialo is in {}", bin.display());
    println!("and in your launcher ({})", desktop.display());
    if linked {
        println!("tel:, sip: and callto: links now open in Dialo");
    } else {
        println!("xdg-mime is missing: tel: and sip: links won't open Dialo");
    }
    if !std::env::var("PATH").unwrap_or_default().split(':').any(|p| PathBuf::from(p) == bin.parent().unwrap()) {
        println!("Note: {} is not on your PATH", bin.parent().unwrap().display());
    }
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    let (bin, desktop) = places()?;
    for p in [&bin, &desktop] {
        match std::fs::remove_file(p) {
            Ok(()) => println!("Removed {}", p.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", p.display())),
        }
    }
    let _ = std::process::Command::new("update-desktop-database").arg(desktop.parent().unwrap()).status();
    Ok(())
}

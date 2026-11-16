//! L'entrée « Compresser avec Video Compressor » du clic droit, et l'ouverture des fichiers produits.
//!
//! - Windows : une commande dans le registre de l'utilisateur, pour chaque extension vidéo. Sous
//!   Windows 11 elle apparaît dans « Afficher d'autres options » (ou avec Maj + clic droit).
//! - Linux : une entrée d'application (« Ouvrir avec »), un script Nautilus, une action Nemo et
//!   un menu de service Dolphin.
//! - macOS : une action rapide du Finder (un service Automator qui lance l'application).
//!
//! Dans les trois cas, l'entrée lance l'exécutable avec les fichiers choisis ; `instance.rs` les
//! réunit dans une seule fenêtre.

use std::path::{Path, PathBuf};

#[allow(unused_imports)]
use crate::engine::VIDEO_EXTS;
use crate::i18n::{tf, tr};

/// Une commande sans fenêtre de console (sous Windows, chaque programme lancé en ouvrirait une).
#[allow(dead_code)]
fn command(program: &str) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Le texte de l'entrée du menu.
pub fn menu_label() -> &'static str {
    tr("Compresser avec Video Compressor")
}

/// Où trouver l'entrée, sur ce système.
pub fn menu_hint() -> &'static str {
    if cfg!(windows) {
        tr("Sous Windows 11, l'entrée se trouve dans « Afficher d'autres options », ou avec Maj et clic droit.")
    } else if cfg!(target_os = "macos") {
        tr("Dans le Finder, l'entrée se trouve dans le clic droit, sous Actions rapides ou Services.")
    } else {
        tr("L'entrée apparaît dans Fichiers (sous Scripts), Nemo et Dolphin, et dans « Ouvrir avec ».")
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".into()))
}

/// L'exécutable que le menu lancera. Une version publiée est d'abord copiée à un endroit stable,
/// pour que le menu marche encore si le fichier téléchargé est déplacé ou effacé.
fn stable_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // Sous macOS l'exécutable vit dans `Video Compressor.app` : on ne le sort pas de là.
    if crate::release::is_dev_build() || cfg!(target_os = "macos") {
        return Ok(exe);
    }
    let dest = if cfg!(windows) {
        let local = std::env::var("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|_| home().join("AppData").join("Local"));
        local.join("Programs").join("Video Compressor").join("video-compressor.exe")
    } else {
        home().join(".local").join("bin").join("video-compressor")
    };
    if dest == exe {
        return Ok(exe);
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    crate::release::replace_exe(&dest, &exe)?;
    Ok(dest)
}

// ---- Windows -----------------------------------------------------------------------------------

#[cfg(windows)]
mod sys {
    use super::*;
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    const VERB: &str = "VideoCompressor";
    /// Les associations par extension, valables quel que soit le programme qui ouvre le fichier.
    const ROOT: &str = r"Software\Classes\SystemFileAssociations";

    fn key(root: &str, ext: &str) -> String {
        format!(r"{root}\.{ext}\shell\{VERB}")
    }

    pub fn registered_in(root: &str) -> bool {
        RegKey::predef(HKEY_CURRENT_USER).open_subkey(format!(r"{}\command", key(root, "mp4"))).is_ok()
    }

    /// Une commande par extension vidéo, sous `root` (le registre de l'utilisateur).
    pub fn register_in(root: &str, exe: &str) -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for ext in VIDEO_EXTS {
            let (verb, _) = hkcu.create_subkey(key(root, ext)).map_err(|e| e.to_string())?;
            verb.set_value("", &menu_label()).map_err(|e| e.to_string())?;
            verb.set_value("Icon", &format!("\"{exe}\",0")).map_err(|e| e.to_string())?;
            let (cmd, _) = verb.create_subkey("command").map_err(|e| e.to_string())?;
            cmd.set_value("", &format!("\"{exe}\" \"%1\"")).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn unregister_in(root: &str) -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for ext in VIDEO_EXTS {
            match hkcu.delete_subkey_all(key(root, ext)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }

    pub fn registered() -> bool {
        registered_in(ROOT)
    }

    pub fn register(exe: &Path) -> Result<(), String> {
        let exe = exe.display().to_string();
        register_in(ROOT, &exe)?;
        shortcut(&exe);
        Ok(())
    }

    pub fn unregister() -> Result<(), String> {
        unregister_in(ROOT)?;
        if let Some(link) = shortcut_path() {
            let _ = std::fs::remove_file(link);
        }
        Ok(())
    }

    fn shortcut_path() -> Option<PathBuf> {
        let appdata = std::env::var("APPDATA").ok()?;
        Some(PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs\Video Compressor.lnk"))
    }

    /// Un raccourci dans le menu Démarrer (au mieux : son absence ne gêne rien).
    fn shortcut(exe: &str) {
        let Some(link) = shortcut_path() else { return };
        if crate::release::is_dev_build() {
            return;
        }
        let quote = |s: &str| s.replace('\'', "''");
        let script = format!("$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.Save()", quote(&link.display().to_string()), quote(exe));
        let _ = command("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]).spawn();
    }

    pub fn reveal(path: &Path) {
        use std::os::windows::process::CommandExt;
        let _ = command("explorer").raw_arg(format!("/select,\"{}\"", path.display())).spawn();
    }

    pub fn open(path: &Path) {
        let _ = command("explorer").arg(path).spawn();
    }
}

// ---- Linux -------------------------------------------------------------------------------------

#[cfg(all(unix, not(target_os = "macos")))]
mod sys {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const MIME: &str = "video/*;image/gif;image/webp;";

    fn data() -> PathBuf {
        match std::env::var("XDG_DATA_HOME") {
            Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => home().join(".local").join("share"),
        }
    }

    /// Les fichiers que l'application pose : l'entrée d'application, puis une entrée par
    /// gestionnaire de fichiers.
    fn files(data: &Path) -> [PathBuf; 4] {
        [
            data.join("applications").join("video-compressor.desktop"),
            data.join("nautilus").join("scripts").join(menu_label()),
            data.join("nemo").join("actions").join("video-compressor.nemo_action"),
            data.join("kio").join("servicemenus").join("video-compressor.desktop"),
        ]
    }

    fn write(path: &Path, text: &str, executable: bool) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, text).map_err(|e| e.to_string())?;
        let mode = if executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|e| e.to_string())
    }

    pub fn registered() -> bool {
        registered_in(&data())
    }

    pub fn register(exe: &Path) -> Result<(), String> {
        register_in(&data(), exe)?;
        let _ = command("update-desktop-database").arg(data().join("applications")).spawn();
        Ok(())
    }

    pub fn unregister() -> Result<(), String> {
        unregister_in(&data())
    }

    pub fn registered_in(data: &Path) -> bool {
        files(data)[0].is_file()
    }

    /// Pose les fichiers sous `data` (le dossier de données de l'utilisateur).
    pub fn register_in(data: &Path, exe: &Path) -> Result<(), String> {
        let exe = exe.display().to_string();
        let label = menu_label();
        let [desktop, nautilus, nemo, dolphin] = files(data);
        let icon = data.join("icons").join("hicolor").join("256x256").join("apps").join("video-compressor.png");
        if let Some(dir) = icon.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&icon, crate::ICON_PNG).map_err(|e| e.to_string())?;
        write(
            &desktop,
            &format!("[Desktop Entry]\nType=Application\nName=Video Compressor\nComment={label}\nExec=\"{exe}\" %F\nIcon=video-compressor\nTerminal=false\nCategories=AudioVideo;Video;\nMimeType={MIME}\nStartupWMClass=video-compressor\n"),
            false,
        )?;
        write(&nautilus, &format!("#!/bin/sh\nexec \"{exe}\" \"$@\"\n"), true)?;
        let exts = VIDEO_EXTS.join(";");
        write(&nemo, &format!("[Nemo Action]\nName={label}\nComment={label}\nExec=\"{exe}\" %F\nIcon-Name=video-compressor\nSelection=notnone\nExtensions={exts};\n"), false)?;
        // Dolphin n'accepte un menu de service que s'il est exécutable.
        write(&dolphin, &format!("[Desktop Entry]\nType=Service\nMimeType={MIME}\nActions=compress\nX-KDE-Priority=TopLevel\n\n[Desktop Action compress]\nName={label}\nIcon=video-compressor\nExec=\"{exe}\" %F\n"), true)
    }

    pub fn unregister_in(data: &Path) -> Result<(), String> {
        for file in files(data) {
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }

    pub fn reveal(path: &Path) {
        let _ = command("xdg-open").arg(path.parent().unwrap_or(path)).spawn();
    }

    pub fn open(path: &Path) {
        let _ = command("xdg-open").arg(path).spawn();
    }
}

// ---- macOS -------------------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod sys {
    use super::*;

    fn workflow() -> PathBuf {
        home().join("Library").join("Services").join(format!("{}.workflow", menu_label()))
    }

    fn xml(text: &str) -> String {
        text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    const HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n";

    /// `Info.plist` du service : son nom dans le menu, et les fichiers pour lesquels il apparaît.
    pub fn info_plist(label: &str) -> String {
        format!(
            "{HEADER}<dict>\n\t<key>NSServices</key>\n\t<array>\n\t\t<dict>\n\t\t\t<key>NSMenuItem</key>\n\t\t\t<dict>\n\t\t\t\t<key>default</key>\n\t\t\t\t<string>{}</string>\n\t\t\t</dict>\n\t\t\t<key>NSMessage</key>\n\t\t\t<string>runWorkflowAsService</string>\n\t\t\t<key>NSSendFileTypes</key>\n\t\t\t<array>\n\t\t\t\t<string>public.movie</string>\n\t\t\t\t<string>public.image</string>\n\t\t\t</array>\n\t\t</dict>\n\t</array>\n</dict>\n</plist>\n",
            xml(label)
        )
    }

    /// `document.wflow` : une seule action Automator, « Exécuter un script shell », qui lance
    /// l'application avec les fichiers choisis en arguments.
    pub fn document(exe: &str) -> String {
        let script = xml(&format!("\"{exe}\" \"$@\" >/dev/null 2>&1 &"));
        format!(
            "{HEADER}<dict>\n\t<key>AMApplicationBuild</key>\n\t<string>523</string>\n\t<key>AMApplicationVersion</key>\n\t<string>2.10</string>\n\t<key>AMDocumentVersion</key>\n\t<string>2</string>\n\t<key>actions</key>\n\t<array>\n\t\t<dict>\n\t\t\t<key>action</key>\n\t\t\t<dict>\n\t\t\t\t<key>AMAccepts</key>\n\t\t\t\t<dict>\n\t\t\t\t\t<key>Container</key>\n\t\t\t\t\t<string>List</string>\n\t\t\t\t\t<key>Optional</key>\n\t\t\t\t\t<true/>\n\t\t\t\t\t<key>Types</key>\n\t\t\t\t\t<array>\n\t\t\t\t\t\t<string>com.apple.cocoa.string</string>\n\t\t\t\t\t</array>\n\t\t\t\t</dict>\n\t\t\t\t<key>AMActionVersion</key>\n\t\t\t\t<string>2.0.3</string>\n\t\t\t\t<key>AMApplication</key>\n\t\t\t\t<array>\n\t\t\t\t\t<string>Automator</string>\n\t\t\t\t</array>\n\t\t\t\t<key>AMParameterProperties</key>\n\t\t\t\t<dict>\n\t\t\t\t\t<key>COMMAND_STRING</key>\n\t\t\t\t\t<dict/>\n\t\t\t\t\t<key>CheckedForUserDefaultShell</key>\n\t\t\t\t\t<dict/>\n\t\t\t\t\t<key>inputMethod</key>\n\t\t\t\t\t<dict/>\n\t\t\t\t\t<key>shell</key>\n\t\t\t\t\t<dict/>\n\t\t\t\t\t<key>source</key>\n\t\t\t\t\t<dict/>\n\t\t\t\t</dict>\n\t\t\t\t<key>AMProvides</key>\n\t\t\t\t<dict>\n\t\t\t\t\t<key>Container</key>\n\t\t\t\t\t<string>List</string>\n\t\t\t\t\t<key>Types</key>\n\t\t\t\t\t<array>\n\t\t\t\t\t\t<string>com.apple.cocoa.string</string>\n\t\t\t\t\t</array>\n\t\t\t\t</dict>\n\t\t\t\t<key>ActionBundlePath</key>\n\t\t\t\t<string>/System/Library/Automator/Run Shell Script.action</string>\n\t\t\t\t<key>ActionName</key>\n\t\t\t\t<string>Run Shell Script</string>\n\t\t\t\t<key>ActionParameters</key>\n\t\t\t\t<dict>\n\t\t\t\t\t<key>COMMAND_STRING</key>\n\t\t\t\t\t<string>{script}</string>\n\t\t\t\t\t<key>CheckedForUserDefaultShell</key>\n\t\t\t\t\t<true/>\n\t\t\t\t\t<key>inputMethod</key>\n\t\t\t\t\t<integer>1</integer>\n\t\t\t\t\t<key>shell</key>\n\t\t\t\t\t<string>/bin/sh</string>\n\t\t\t\t\t<key>source</key>\n\t\t\t\t\t<string></string>\n\t\t\t\t</dict>\n\t\t\t\t<key>BundleIdentifier</key>\n\t\t\t\t<string>com.apple.RunShellScript</string>\n\t\t\t\t<key>CFBundleVersion</key>\n\t\t\t\t<string>2.0.3</string>\n\t\t\t\t<key>CanShowSelectedItemsWhenRun</key>\n\t\t\t\t<false/>\n\t\t\t\t<key>CanShowWhenRun</key>\n\t\t\t\t<true/>\n\t\t\t\t<key>Category</key>\n\t\t\t\t<array>\n\t\t\t\t\t<string>AMCategoryUtilities</string>\n\t\t\t\t</array>\n\t\t\t\t<key>Class Name</key>\n\t\t\t\t<string>RunShellScriptAction</string>\n\t\t\t\t<key>InputUUID</key>\n\t\t\t\t<string>6B2D0C2E-7C0B-4E0E-9B55-2F5A2C4E7A11</string>\n\t\t\t\t<key>OutputUUID</key>\n\t\t\t\t<string>0E7C6C1A-53B1-4C55-8A39-0B8E6A0F3D22</string>\n\t\t\t\t<key>UUID</key>\n\t\t\t\t<string>C4B0E2A7-1D7E-4F0B-9B0A-6D2F7E5A9C33</string>\n\t\t\t\t<key>UnlocalizedApplications</key>\n\t\t\t\t<array>\n\t\t\t\t\t<string>Automator</string>\n\t\t\t\t</array>\n\t\t\t\t<key>arguments</key>\n\t\t\t\t<dict/>\n\t\t\t</dict>\n\t\t</dict>\n\t</array>\n\t<key>connectors</key>\n\t<dict/>\n\t<key>workflowMetaData</key>\n\t<dict>\n\t\t<key>serviceInputTypeIdentifier</key>\n\t\t<string>com.apple.Automator.fileSystemObject</string>\n\t\t<key>serviceOutputTypeIdentifier</key>\n\t\t<string>com.apple.Automator.nothing</string>\n\t\t<key>serviceProcessesInput</key>\n\t\t<integer>0</integer>\n\t\t<key>workflowTypeIdentifier</key>\n\t\t<string>com.apple.Automator.servicesMenu</string>\n\t</dict>\n</dict>\n</plist>\n"
        )
    }

    pub fn registered() -> bool {
        workflow().join("Contents").join("document.wflow").is_file()
    }

    pub fn register(exe: &Path) -> Result<(), String> {
        let contents = workflow().join("Contents");
        std::fs::create_dir_all(&contents).map_err(|e| e.to_string())?;
        std::fs::write(contents.join("Info.plist"), info_plist(menu_label())).map_err(|e| e.to_string())?;
        std::fs::write(contents.join("document.wflow"), document(&exe.display().to_string())).map_err(|e| e.to_string())?;
        // Le Finder relit la liste des services.
        let _ = command("/System/Library/CoreServices/pbs").arg("-update").spawn();
        Ok(())
    }

    pub fn unregister() -> Result<(), String> {
        match std::fs::remove_dir_all(workflow()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn reveal(path: &Path) {
        let _ = command("open").arg("-R").arg(path).spawn();
    }

    pub fn open(path: &Path) {
        let _ = command("open").arg(path).spawn();
    }
}

/// L'entrée du clic droit est-elle en place ?
pub fn menu_registered() -> bool {
    sys::registered()
}

/// Ajoute l'entrée au clic droit des vidéos, pour cet utilisateur (aucun droit d'administrateur).
pub fn register_menu() -> Result<(), String> {
    let exe = stable_exe()?;
    sys::register(&exe).map_err(|e| tf("Le clic droit n'a pas pu être ajouté ({0})", &[("0", &e)]))
}

pub fn unregister_menu() -> Result<(), String> {
    sys::unregister().map_err(|e| tf("Le clic droit n'a pas pu être retiré ({0})", &[("0", &e)]))
}

/// Montre le fichier dans son dossier.
pub fn reveal(path: &Path) {
    sys::reveal(path);
}

/// Ouvre le fichier avec le programme par défaut.
pub fn open(path: &Path) {
    sys::open(path);
}

#[cfg(test)]
mod tests {
    /// Les deux fichiers du service du Finder sont des listes de propriétés que macOS accepte.
    #[cfg(target_os = "macos")]
    #[test]
    fn service_du_finder_bien_forme() {
        let doc = super::sys::document("/Applications/Video Compressor.app/Contents/MacOS/video-compressor");
        assert!(!doc.contains("&quot;") && doc.contains("\"$@\"") && doc.ends_with("</plist>\n"));
        let info = super::sys::info_plist("Compresser & convertir");
        assert!(info.contains("Compresser &amp; convertir"));
        let dir = std::env::temp_dir().join(format!("vc-service-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in [("document.wflow", doc), ("Info.plist", info)] {
            let path = dir.join(name);
            std::fs::write(&path, text).unwrap();
            let out = std::process::Command::new("plutil").arg("-lint").arg(&path).output().unwrap();
            assert!(out.status.success(), "{name} : {}", String::from_utf8_lossy(&out.stdout));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L'entrée s'écrit et se retire du registre (dans une branche d'essai, effacée à la fin).
    #[cfg(windows)]
    #[test]
    fn entree_du_registre_ecrite_puis_retiree() {
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;
        let root = format!(r"Software\Classes\VideoCompressorTest{}", std::process::id());
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        assert!(!super::sys::registered_in(&root));
        super::sys::register_in(&root, r"C:\Outils\video-compressor.exe").unwrap();
        assert!(super::sys::registered_in(&root));
        let verb = hkcu.open_subkey(format!(r"{root}\.webp\shell\VideoCompressor")).unwrap();
        assert_eq!(verb.get_value::<String, _>("").unwrap(), super::menu_label());
        let command: String = verb.open_subkey("command").unwrap().get_value("").unwrap();
        assert_eq!(command, r#""C:\Outils\video-compressor.exe" "%1""#);
        super::sys::unregister_in(&root).unwrap();
        assert!(!super::sys::registered_in(&root));
        hkcu.delete_subkey_all(&root).unwrap();
    }

    /// Sous Linux, les quatre fichiers sont posés dans le dossier de données, puis retirés.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn entrees_linux_ecrites_puis_retirees() {
        let dir = std::env::temp_dir().join(format!("vc-xdg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!super::sys::registered_in(&dir));
        super::sys::register_in(&dir, std::path::Path::new("/opt/video compressor/video-compressor")).unwrap();
        assert!(super::sys::registered_in(&dir));
        let desktop = std::fs::read_to_string(dir.join("applications/video-compressor.desktop")).unwrap();
        assert!(desktop.contains("Exec=\"/opt/video compressor/video-compressor\" %F") && desktop.contains("MimeType=video/*;"), "{desktop}");
        let script = dir.join("nautilus/scripts").join(super::menu_label());
        assert!(std::fs::read_to_string(&script).unwrap().contains("exec \"/opt/video compressor/video-compressor\" \"$@\""));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&script).unwrap().permissions().mode() & 0o111, 0o111, "le script Nautilus est exécutable");
        assert!(dir.join("nemo/actions/video-compressor.nemo_action").is_file() && dir.join("kio/servicemenus/video-compressor.desktop").is_file());
        assert!(dir.join("icons/hicolor/256x256/apps/video-compressor.png").is_file());
        super::sys::unregister_in(&dir).unwrap();
        assert!(!super::sys::registered_in(&dir) && !script.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn le_texte_du_menu_existe() {
        assert!(!super::menu_label().is_empty() && !super::menu_hint().is_empty());
    }
}

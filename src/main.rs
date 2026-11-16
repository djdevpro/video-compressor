//! Video Compressor : compresser et convertir une vidéo d'un clic droit, en egui.
//!
//! Interface :
//! - `theme.rs`, `icons.rs`, `widgets.rs` : apparence et boîte à widgets (repris de Rust Project).
//! - `app.rs`      : la fenêtre (fichiers, réglages), les actions, les messages des fils de travail.
//! - `dialogs.rs`  : préférences et mise à jour.
//!
//! Moteur :
//! - `settings.rs` : les réglages d'encodage, les préréglages, les préférences enregistrées.
//! - `engine.rs`   : FFmpeg lié dans l'exécutable : mesurer un fichier, le décoder, le filtrer,
//!   l'encoder.
//! - `webp.rs`     : les WebP animés, que FFmpeg ne sait pas lire.
//! - `queue.rs`    : la file des fichiers et ses fils (analyse, encodage).
//! - `shell.rs`    : l'entrée du clic droit (Windows, Linux, macOS).
//! - `instance.rs` : une seule fenêtre, même lancée dix fois.
//! - `release.rs`, `net.rs` : la mise à jour de l'application.

// Sous Windows, l'application s'ouvre sans console. Lancée depuis un terminal, elle y écrit ses
// messages (voir `attach_parent_console`).
#![cfg_attr(not(test), windows_subsystem = "windows")]

mod app;
mod dialogs;
mod draw;
mod engine;
mod i18n;
mod icons;
mod instance;
mod net;
mod queue;
mod release;
mod settings;
mod shell;
mod theme;
mod widgets;
mod webp;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

/// L'icône de l'application (fenêtre, barre de titre, entrée d'application sous Linux).
pub const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

fn main() -> eframe::Result {
    #[cfg(windows)]
    attach_parent_console();
    // SVT-AV1 annonce chaque encodage sur la sortie d'erreur : seulement ses erreurs.
    // SAFETY : tout début du programme, avant le moindre autre fil d'exécution.
    unsafe { std::env::set_var("SVT_LOG", "1") };
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `video-compressor --version` : sert de test de fumée à la CI.
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("video-compressor {} (FFmpeg {})", release::VERSION, engine::version());
        return Ok(());
    }
    // Les fichiers passés par le clic droit (tout ce qui n'est pas une option).
    let paths: Vec<PathBuf> = args.iter().filter(|a| !a.starts_with("--")).map(PathBuf::from).map(|p| std::path::absolute(&p).unwrap_or(p)).collect();
    // Une fenêtre est déjà ouverte : elle a reçu les fichiers, on s'arrête là.
    let instance::Role::Primary(inbox, waker) = instance::start(&paths) else { return Ok(()) };

    // L'icône de la fenêtre (barre des tâches, Alt+Tab) ; sans elle, eframe met la sienne.
    let icon = eframe::icon_data::from_png_bytes(ICON_PNG).expect("icône de la fenêtre");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Video Compressor")
            .with_app_id("video-compressor")
            .with_icon(icon)
            .with_inner_size([1080.0, 740.0])
            .with_min_inner_size([960.0, 680.0])
            .with_decorations(false)
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("video-compressor", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, Some((inbox, waker)), paths)))))
}

/// Lancée depuis un terminal (`cargo run`…), l'application y écrit ses messages ; d'un double-clic,
/// il n'y a pas de terminal et rien ne s'ouvre.
#[cfg(windows)]
fn attach_parent_console() {
    unsafe extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    // SAFETY : simple appel système, sans pointeur ; un échec (pas de terminal parent) est normal.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

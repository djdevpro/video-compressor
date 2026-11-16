//! Une seule fenêtre. Sélectionner dix vidéos puis faire un clic droit lance dix fois le programme :
//! le premier ouvre la fenêtre et écoute sur un port local, les suivants lui passent leur fichier
//! et se ferment aussitôt.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Un port de la machine seulement (127.0.0.1) : rien ne sort sur le réseau.
pub const PORT: u16 = 47_611;
const HELLO: &str = "video-compressor 1";
const OK: &str = "ok";

/// De quoi réveiller la fenêtre quand des fichiers arrivent : le contexte egui, connu après son
/// ouverture.
#[derive(Clone, Default)]
pub struct Waker(Arc<Mutex<Option<egui::Context>>>);

impl Waker {
    pub fn set(&self, ctx: &egui::Context) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(ctx.clone());
        }
    }

    fn wake(&self) {
        if let Ok(slot) = self.0.lock()
            && let Some(ctx) = slot.as_ref()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.request_repaint();
        }
    }
}

pub enum Role {
    /// Cette instance ouvre la fenêtre, et reçoit les fichiers des suivantes.
    Primary(Receiver<Vec<PathBuf>>, Waker),
    /// Une fenêtre est déjà ouverte : elle a reçu les fichiers, celle-ci n'a plus rien à faire.
    Forwarded,
}

fn addr(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

fn serve(listener: TcpListener, tx: Sender<Vec<PathBuf>>, waker: Waker) {
    for stream in listener.incoming().flatten() {
        let mut stream = stream;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut text = String::new();
        if stream.read_to_string(&mut text).is_err() {
            continue;
        }
        let mut lines = text.lines();
        if lines.next() != Some(HELLO) {
            continue;
        }
        let paths: Vec<PathBuf> = lines.filter(|l| !l.is_empty()).map(PathBuf::from).collect();
        let _ = stream.write_all(OK.as_bytes());
        if tx.send(paths).is_err() {
            return;
        }
        waker.wake();
    }
}

/// Passe les fichiers à la fenêtre déjà ouverte. `false` si personne ne répond comme elle.
fn forward(port: u16, paths: &[PathBuf]) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&addr(port), Duration::from_secs(1)) else { return false };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let mut message = format!("{HELLO}\n");
    for p in paths {
        message.push_str(&p.to_string_lossy());
        message.push('\n');
    }
    if stream.write_all(message.as_bytes()).is_err() || stream.shutdown(Shutdown::Write).is_err() {
        return false;
    }
    let mut reply = String::new();
    stream.read_to_string(&mut reply).is_ok() && reply == OK
}

/// Devient la fenêtre principale, ou passe `paths` à celle qui existe.
pub fn start_on(port: u16, paths: &[PathBuf]) -> Role {
    let (tx, rx) = channel();
    let waker = Waker::default();
    match TcpListener::bind(addr(port)) {
        Ok(listener) => {
            let w = waker.clone();
            std::thread::spawn(move || serve(listener, tx, w));
            Role::Primary(rx, waker)
        }
        // Le port est pris : par une fenêtre déjà ouverte (on lui passe les fichiers), ou par un
        // autre programme (on ouvre alors une fenêtre indépendante, sans écoute).
        Err(_) if forward(port, paths) => Role::Forwarded,
        Err(_) => Role::Primary(rx, waker),
    }
}

pub fn start(paths: &[PathBuf]) -> Role {
    start_on(PORT, paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_deuxieme_instance_passe_ses_fichiers_a_la_premiere() {
        // Un port libre, pour ne pas gêner une vraie fenêtre ouverte pendant les tests.
        let port = TcpListener::bind(addr(0)).unwrap().local_addr().unwrap().port();
        let Role::Primary(rx, _waker) = start_on(port, &[]) else { panic!("la première instance doit ouvrir la fenêtre") };
        let files = [PathBuf::from(r"C:\Vidéos\été 2026.mp4"), PathBuf::from("b.webp")];
        assert!(matches!(start_on(port, &files), Role::Forwarded));
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), files);
        assert!(matches!(start_on(port, &[]), Role::Forwarded), "sans fichier : la fenêtre est seulement ramenée devant");
        assert!(rx.recv_timeout(Duration::from_secs(3)).unwrap().is_empty());
    }

    #[test]
    fn un_port_pris_par_un_autre_programme_n_empeche_pas_d_ouvrir() {
        let other = TcpListener::bind(addr(0)).unwrap();
        let port = other.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut s in other.incoming().flatten() {
                let _ = s.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
            }
        });
        assert!(matches!(start_on(port, &[PathBuf::from("a.mp4")]), Role::Primary(..)));
    }
}

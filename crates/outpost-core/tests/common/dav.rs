//! A private Radicale (a small CalDAV and CardDAV server) for a test: its
//! own port and storage, one user, gone when the test ends.
//!
//! Radicale is a Python package. It is looked for in the system Python and
//! in the folder named by `OUTPOST_RADICALE` (default `~/outpost-dav`, where
//! `pip install --target ~/outpost-dav radicale` puts it). Without it the
//! tests skip, unless `OUTPOST_REQUIRE_RADICALE` is set.

use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

pub const DAV_USER: &str = "ann";
pub const DAV_PASSWORD: &str = "dav-secret";

pub struct DavServer {
    child: Child,
    dir: PathBuf,
    pub port: u16,
}

fn python_path() -> String {
    let own = std::env::var("OUTPOST_RADICALE").unwrap_or_else(|_| {
        let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
        format!("{home}/outpost-dav")
    });
    match std::env::var("PYTHONPATH") {
        Ok(existing) if !existing.is_empty() => format!("{own}:{existing}"),
        _ => own,
    }
}

fn python() -> Option<&'static str> {
    ["python3", "python"].into_iter().find(|python| {
        Command::new(python)
            .args(["-c", "import radicale"])
            .env("PYTHONPATH", python_path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

impl DavServer {
    /// Starts a server, or returns `None` (after saying so) if Radicale is
    /// not installed on this machine.
    pub fn start() -> Option<DavServer> {
        let Some(python) = python() else {
            if std::env::var_os("OUTPOST_REQUIRE_RADICALE").is_some() {
                panic!("Radicale is required for this test run but is not installed");
            }
            eprintln!("SKIPPED: Radicale is not installed, so this calendar and contacts test did not run");
            return None;
        };
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("outpost-radicale-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("storage")).unwrap();
        std::fs::write(dir.join("users"), format!("{DAV_USER}:{DAV_PASSWORD}\n")).unwrap();
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let child = Command::new(python)
            .args(["-m", "radicale"])
            .arg(format!("--server-hosts=127.0.0.1:{port}"))
            .arg(format!("--storage-filesystem-folder={}", dir.join("storage").display()))
            .arg("--auth-type=htpasswd")
            .arg(format!("--auth-htpasswd-filename={}", dir.join("users").display()))
            .arg("--auth-htpasswd-encryption=plain")
            .arg("--auth-delay=0")
            .env("PYTHONPATH", python_path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("python can be started");
        let server = DavServer { child, dir, port };
        let deadline = Instant::now() + Duration::from_secs(20);
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(Instant::now() < deadline, "Radicale did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        Some(server)
    }

    /// The address a user would type: the server, with no path.
    pub fn address(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }
}

impl Drop for DavServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

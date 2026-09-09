//! Small loopback-only HTTP host. Certificate checking runs off the request thread.
use crate::{certificate::Certificate, player::Game};
use anyhow::{ensure, Context, Result};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

struct App {
    game: Option<Session>,
    certificate: Option<Arc<Certificate>>,
    phase: &'static str,
    message: String,
    revision: u64,
    generation: u64,
    busy: bool,
    error: Option<String>,
    cancel: Arc<AtomicBool>,
    analysis_slots: Arc<AtomicUsize>,
    visitor_session: bool,
}
#[derive(Clone)]
enum Session {
    Verified(Game),
    Practice(crate::trainer::Trainer),
    Demo(crate::player::Demo),
}
impl Session {
    fn state(&self) -> Value {
        match self {
            Self::Verified(game) => game.state(),
            Self::Practice(game) => game.state(),
            Self::Demo(game) => game.state(),
        }
    }
    fn mode(&self) -> &'static str {
        match self {
            Self::Verified(_) => "verified",
            Self::Practice(_) => "play-first",
            Self::Demo(_) => "demo",
        }
    }
}
impl App {
    fn new() -> Self {
        Self {
            game: None,
            certificate: None,
            phase: "waiting",
            message:
                "Waiting for a strategy certificate. You can still choose Play first to practice."
                    .into(),
            revision: 0,
            generation: 0,
            busy: false,
            error: None,
            cancel: Arc::new(AtomicBool::new(false)),
            analysis_slots: Arc::new(AtomicUsize::new(0)),
            visitor_session: false,
        }
    }
    fn state(&self) -> Value {
        let mut value = self
            .game
            .as_ref()
            .map(Session::state)
            .unwrap_or_else(|| json!({"phase":self.phase,"message":self.message}));
        value["revision"] = json!(self.revision);
        value["mode"] = json!(self.game.as_ref().map_or("verified", Session::mode));
        value["thinking"] = json!(self.busy);
        value["visitorSession"] = json!(self.visitor_session);
        value["certificateReady"] = json!(self.certificate.is_some());
        value["demoAvailable"] = json!(self
            .certificate
            .as_ref()
            .is_some_and(|cert| cert.header.forced_depth.is_some()));
        value["analysisError"] = json!(self.error);
        value
    }
}
struct AnalysisPermit(Arc<AtomicUsize>);
impl AnalysisPermit {
    fn reserve(slots: Arc<AtomicUsize>) -> Result<Self> {
        slots
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |n| {
                (n < 4).then_some(n + 1)
            })
            .map_err(|_| {
                anyhow::anyhow!("The computer is busy with other games. Please try again shortly.")
            })?;
        Ok(Self(slots))
    }
}
impl Drop for AnalysisPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
struct Visitor {
    app: Arc<Mutex<App>>,
    last_seen: Instant,
}
struct Hub {
    template: Arc<Mutex<App>>,
    visitors: Mutex<HashMap<String, Visitor>>,
    origin_file: Option<PathBuf>,
}
impl Hub {
    fn public_origin(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.origin_file.as_ref()?).ok()?;
        let text = text.trim();
        let host = text.strip_prefix("https://")?;
        if text.len() > 300
            || !host.contains('.')
            || !host.split('.').all(|label| {
                !label.is_empty()
                    && label
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-')
                    && label.as_bytes()[0].is_ascii_alphanumeric()
                    && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
            })
        {
            return None;
        }
        Some(text.to_string())
    }
    fn visitor(&self, cookie: Option<&str>) -> Result<(String, bool, Arc<Mutex<App>>)> {
        let supplied = cookie.and_then(|cookie| {
            cookie
                .split(';')
                .find_map(|pair| pair.trim().strip_prefix("__Host-gekitai_session="))
        });
        let now = Instant::now();
        let (id, fresh, app) = {
            let mut visitors = self.visitors.lock();
            visitors.retain(|_, visitor| {
                if now.duration_since(visitor.last_seen) > Duration::from_secs(3600) {
                    visitor.app.lock().cancel.store(true, Ordering::Relaxed);
                    false
                } else {
                    true
                }
            });
            if let Some((id, visitor)) =
                supplied.and_then(|id| visitors.get_mut(id).map(|visitor| (id, visitor)))
            {
                visitor.last_seen = now;
                (id.to_string(), false, visitor.app.clone())
            } else {
                ensure!(
                    visitors.len() < 256,
                    "Too many active games. Please try again later."
                );
                let mut random = [0u8; 24];
                std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
                let id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
                let mut app = App::new();
                app.visitor_session = true;
                app.analysis_slots = self.template.lock().analysis_slots.clone();
                let app = Arc::new(Mutex::new(app));
                visitors.insert(
                    id.clone(),
                    Visitor {
                        app: app.clone(),
                        last_seen: now,
                    },
                );
                (id, true, app)
            }
        };
        let template = self.template.lock();
        let mut game = app.lock();
        if game.certificate.is_none() {
            if let Some(certificate) = &template.certificate {
                game.certificate = Some(certificate.clone());
                if game.game.is_none() {
                    game.game = Some(Session::Verified(Game::new(certificate.clone())?));
                }
                game.phase = "ready";
                game.revision += 1;
            } else if game.phase != template.phase || game.message != template.message {
                game.phase = template.phase;
                game.message = template.message.clone();
                game.revision += 1;
            }
        }
        drop(game);
        drop(template);
        Ok((id, fresh, app))
    }
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
    cookie: Option<String>,
}
fn request(stream: &mut impl Read, port: u16, public_origin: Option<&str>) -> Result<Request> {
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        ensure!(header.len() < 8192, "Request header too large");
        let mut byte = [0];
        stream.read_exact(&mut byte)?;
        header.push(byte[0]);
    }
    let text = std::str::from_utf8(&header)?;
    let mut lines = text.split("\r\n");
    let mut first = lines.next().context("Missing request")?.split_whitespace();
    let method = first.next().context("Missing method")?.to_string();
    let path = first.next().context("Missing path")?.to_string();
    ensure!(
        first.next() == Some("HTTP/1.1") && first.next().is_none(),
        "Unsupported HTTP request"
    );
    let mut host = None;
    let mut origin = None;
    let mut length = None;
    let mut content_type = None;
    let mut cookie = None;
    for line in lines.filter(|s| !s.is_empty()) {
        let (name, value) = line.split_once(':').context("Invalid header")?;
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "host" => {
                ensure!(host.is_none(), "Duplicate host");
                host = Some(value);
            }
            "origin" => {
                ensure!(origin.is_none(), "Duplicate origin");
                origin = Some(value);
            }
            "content-length" => {
                ensure!(length.is_none(), "Duplicate length");
                length = Some(value.parse::<usize>()?);
            }
            "content-type" => {
                ensure!(content_type.is_none(), "Duplicate content type");
                content_type = Some(value);
            }
            "cookie" => {
                ensure!(cookie.is_none(), "Duplicate cookie header");
                cookie = Some(value.to_string());
            }
            "transfer-encoding" => anyhow::bail!("Transfer encoding is unsupported"),
            _ => (),
        }
    }
    let hosts = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    let host = host.context("Host is required")?;
    ensure!(
        hosts.iter().any(|h| h == host),
        "Only the loopback host is allowed"
    );
    if let Some(origin) = origin {
        ensure!(
            origin == format!("http://{host}") || Some(origin) == public_origin,
            "Origin does not match host"
        );
    }
    if method == "POST" {
        ensure!(origin.is_some(), "Origin is required for a move");
        ensure!(content_type == Some("application/json"), "JSON is required");
    }
    let length = length.unwrap_or(0);
    ensure!(length <= 128, "Request body too large");
    let mut body = vec![0; length];
    stream.read_exact(&mut body)?;
    Ok(Request {
        method,
        path,
        body,
        cookie,
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Action {
    revision: u64,
    square: Option<u8>,
    mode: Option<String>,
}

fn begin_analysis(
    app: &mut App,
    shared: &Arc<Mutex<App>>,
    mut game: Session,
    permit: AnalysisPermit,
) {
    app.busy = true;
    let generation = app.generation;
    let stop = app.cancel.clone();
    let shared = shared.clone();
    std::thread::spawn(move || {
        let _permit = permit;
        let result = match &mut game {
            Session::Practice(game) => game.reply(Duration::from_secs(3), stop.clone()),
            Session::Demo(game) => game.prepare(&stop),
            Session::Verified(_) => unreachable!(),
        };
        let mut app = shared.lock();
        if app.generation != generation || stop.load(Ordering::Relaxed) {
            return;
        }
        app.busy = false;
        app.revision += 1;
        match result {
            Ok(()) => app.game = Some(game),
            Err(error) => app.error = Some(error.to_string()),
        }
    });
}
fn route(req: Request, shared: &Arc<Mutex<App>>) -> Result<(u16, &'static str, String)> {
    if req.method == "GET" && req.path == "/" {
        return Ok((
            200,
            "text/html; charset=utf-8",
            include_str!("player.html").into(),
        ));
    }
    let mut app = shared.lock();
    if req.method == "GET" && req.path == "/api/state" {
        return Ok((200, "application/json", app.state().to_string()));
    }
    if req.method == "POST" && ["/api/move", "/api/reset", "/api/step"].contains(&req.path.as_str())
    {
        let action: Action = serde_json::from_slice(&req.body)?;
        ensure!(
            action.revision == app.revision,
            "Board changed; please try again"
        );
        // Reserve capacity before changing the board. Public visitors share a
        // small analysis budget; a rejected request never leaves a half-move.
        let wants_demo = action
            .mode
            .as_deref()
            .unwrap_or_else(|| app.game.as_ref().map_or("verified", Session::mode))
            == "demo";
        let mut permit = if (req.path == "/api/reset" && wants_demo)
            || (req.path == "/api/move" && matches!(app.game, Some(Session::Practice(_))))
        {
            Some(AnalysisPermit::reserve(app.analysis_slots.clone())?)
        } else {
            None
        };
        if req.path == "/api/reset" {
            let mode = action
                .mode
                .as_deref()
                .unwrap_or_else(|| app.game.as_ref().map_or("verified", Session::mode));
            let game = match mode {
                "verified" => Session::Verified(match app.game.as_ref() {
                    Some(Session::Verified(game)) => game.reset()?,
                    _ => Game::new(
                        app.certificate
                            .clone()
                            .context("The verified strategy is not ready yet")?,
                    )?,
                }),
                "play-first" => Session::Practice(crate::trainer::Trainer::new(
                    app.certificate
                        .as_ref()
                        .map_or_else(crate::rules::Rules::default, |cert| cert.header.rules),
                )),
                "demo" => Session::Demo(crate::player::Demo::new(
                    app.certificate
                        .clone()
                        .context("The verified strategy is not ready yet")?,
                )?),
                _ => anyhow::bail!("Unknown game mode"),
            };
            app.cancel.store(true, Ordering::Relaxed);
            app.cancel = Arc::new(AtomicBool::new(false));
            app.generation += 1;
            app.busy = false;
            app.error = None;
            app.game = Some(game.clone());
            if matches!(game, Session::Demo(_)) {
                begin_analysis(&mut app, shared, game, permit.take().unwrap());
            }
        } else {
            ensure!(!app.busy, "The computer is thinking");
            ensure!(action.mode.is_none(), "Change modes with New game");
            let game = app.game.as_mut().context("Choose a game mode first")?;
            match (&req.path[..], game) {
                ("/api/move", Session::Verified(game)) => {
                    game.human_move(action.square.context("Missing square")?)?
                }
                ("/api/move", Session::Practice(game)) => {
                    game.human_move(action.square.context("Missing square")?)?
                }
                ("/api/step", Session::Demo(game)) => game.step()?,
                _ => anyhow::bail!("This action is not available in the current mode"),
            }
            app.generation += 1;
            if matches!(&app.game,Some(Session::Practice(game)) if game.needs_reply()) {
                let game = app.game.clone().unwrap();
                begin_analysis(&mut app, shared, game, permit.take().unwrap());
            }
        }
        app.revision += 1;
        return Ok((200, "application/json", app.state().to_string()));
    }
    Ok((
        404,
        "application/json",
        json!({"error":"Not found"}).to_string(),
    ))
}
fn respond(mut stream: TcpStream, port: u16, hub: &Hub) -> Result<()> {
    let public_origin = hub.public_origin();
    let mut cookie_header = String::new();
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let response = request(&mut stream, port, public_origin.as_deref()).and_then(|req| {
        let app=if hub.origin_file.is_some() && req.path.starts_with("/api/") {
            let (session, fresh, app)=hub.visitor(req.cookie.as_deref())?;
            if fresh { cookie_header=format!("Set-Cookie: __Host-gekitai_session={session}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=86400\r\n"); }
            app
        } else {hub.template.clone()};
        route(req,&app)
    });
    let (status, kind, body) = response.unwrap_or_else(|e| {
        (
            400,
            "application/json",
            json!({"error":e.to_string()}).to_string(),
        )
    });
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Bad Request",
    };
    write!(stream,"HTTP/1.1 {status} {reason}\r\n{cookie_header}Content-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'\r\nConnection: close\r\n\r\n{body}",body.len())?;
    Ok(())
}
pub fn serve(path: PathBuf, port: u16, origin_file: Option<PathBuf>) -> Result<()> {
    ensure!(port > 0, "Choose a nonzero port");
    let listener = TcpListener::bind(("127.0.0.1", port)).context("Cannot bind player port")?;
    let app = Arc::new(Mutex::new(App::new()));
    let hub = Arc::new(Hub {
        template: app.clone(),
        visitors: Mutex::new(HashMap::new()),
        origin_file,
    });
    let loading = app.clone();
    std::thread::spawn(move || {
        let mut previous = None;
        loop {
            if let Ok(metadata) = std::fs::metadata(&path) {
                let signature = (metadata.len(), metadata.modified().ok());
                if previous != Some(signature) {
                    previous = Some(signature);
                    {
                        let mut app = loading.lock();
                        app.phase = "checking";
                        app.message =
                            "Checking every required reply in the strategy certificate…".into();
                        app.revision += 1;
                    }
                    let checked = Certificate::load(&path).map(Arc::new);
                    let mut app = loading.lock();
                    app.revision += 1;
                    match checked {
                        Ok(certificate) => {
                            if app.game.is_none() {
                                match Game::new(certificate.clone()) {
                                    Ok(game) => app.game = Some(Session::Verified(game)),
                                    Err(error) => app.error = Some(error.to_string()),
                                }
                            }
                            app.certificate = Some(certificate);
                            app.phase = "ready";
                            break;
                        }
                        Err(error) => {
                            eprintln!("Certificate rejected: {error:#}");
                            app.phase = "error";
                            app.message=format!("Certificate verification failed: {error}. Waiting for a replacement.");
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    println!(
        "Browser player: http://127.0.0.1:{port} (public visitor sessions: {})",
        hub.origin_file.is_some()
    );
    // Browsers may open idle preconnections. A small bounded pool keeps one
    // unfinished request from blocking board updates or other tabs.
    let (connections, receiver) = std::sync::mpsc::sync_channel::<TcpStream>(16);
    let receiver = Arc::new(Mutex::new(receiver));
    for _ in 0..4 {
        let receiver = receiver.clone();
        let hub = hub.clone();
        std::thread::spawn(move || loop {
            let stream = receiver.lock().recv();
            let Ok(stream) = stream else {
                break;
            };
            if let Err(error) = respond(stream, port, &hub) {
                eprintln!("Player request: {error}");
            }
        });
    }
    for stream in listener.incoming() {
        let _ = connections.try_send(stream?);
    }
    Ok(())
}
#[cfg(test)]
mod tests;

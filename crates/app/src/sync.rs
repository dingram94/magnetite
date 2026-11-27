//! Sync client (protocol v2, zero knowledge): pushes the sidebar and
//! settings to a `magnetite-sync` server as signed, encrypted blobs and pulls
//! newer versions. Network work runs on a worker thread; results come back to
//! the UI thread through `Engine::post`.
//!
//! The only secret is the sync code (32 random bytes, 24 words). It lives in
//! `<data-dir>/sync.key`, created with mode 0600, never in `state.json`;
//! everything else (signing key, encryption key, collection ids) is derived
//! from it.
//!
//! Versions only go up. A write counts once the server acknowledges exactly
//! the version it was encrypted for, and a server copy older than what this
//! device holds is refused, so a server cannot roll a device back to an older
//! (authentic) ciphertext.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use magnetite_core::SidebarSnapshot;
use magnetite_core::archive::{self, Archive, ArchivedTab};
use magnetite_engine::Engine;
use magnetite_sync_proto::{
    ChainKeys, CollectionResponse, CreateChainRequest, PutCollectionRequest, PutCollectionResponse,
    SyncCode,
};
use magnetite_ui::t;
use serde::{Deserialize, Serialize};

use crate::devices::{self, DEVICES, Devices, Edit, Me, RemoteTab};
use crate::lock;
use crate::state::{Settings, SyncSettings};

pub const SIDEBAR: &str = "sidebar";
pub const SETTINGS: &str = "settings";
pub const ARCHIVE: &str = "archive";

/// Time to set up a connection (TLS included), and for a whole request.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Largest answer read from a sync server: a collection is at most 16 MiB of
/// ciphertext, as JSON a bit more.
const MAX_REPLY: u64 = 32 * 1024 * 1024;

/// Secrets file content (`sync.key`, and `sync.delete` for a chain this
/// device left that the server has not confirmed deleting yet).
#[derive(Serialize, Deserialize, Clone)]
struct Secrets {
    /// Base64 seed of the sync code.
    seed: String,
    /// The server, for a pending deletion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    server: Option<String>,
}

fn secrets_path(data_dir: &Path) -> PathBuf {
    data_dir.join("sync.key")
}

fn pending_delete_path(data_dir: &Path) -> PathBuf {
    data_dir.join("sync.delete")
}

fn load_code(data_dir: &Path) -> Option<SyncCode> {
    let text = std::fs::read_to_string(secrets_path(data_dir)).ok()?;
    let secrets: Secrets = serde_json::from_str(&text).ok()?;
    SyncCode::from_base64(&secrets.seed).ok()
}

/// Write `sync.key` (see `files::write_private`).
fn save_code(data_dir: &Path, code: &SyncCode) -> std::io::Result<()> {
    let secrets = Secrets {
        seed: code.to_base64(),
        server: None,
    };
    crate::files::write_private(&secrets_path(data_dir), &serde_json::to_vec(&secrets)?)
}

/// Remember to delete `code`'s chain on `server` until the server confirms.
fn save_pending_delete(data_dir: &Path, server: &str, code: &SyncCode) -> std::io::Result<()> {
    let secrets = Secrets {
        seed: code.to_base64(),
        server: Some(server.to_string()),
    };
    crate::files::write_private(
        &pending_delete_path(data_dir),
        &serde_json::to_vec(&secrets)?,
    )
}

fn load_pending_delete(data_dir: &Path) -> Option<(String, SyncCode)> {
    let text = std::fs::read_to_string(pending_delete_path(data_dir)).ok()?;
    let secrets: Secrets = serde_json::from_str(&text).ok()?;
    Some((secrets.server?, SyncCode::from_base64(&secrets.seed).ok()?))
}

fn forget_code(data_dir: &Path) {
    std::fs::remove_file(secrets_path(data_dir)).ok();
}

/// The 24 words of this device's sync code, to show the user.
pub fn code_words(data_dir: &Path) -> Option<String> {
    load_code(data_dir).map(|c| c.words())
}

/// What a phone scans to join the chain on `server` with the 24 `words`: a
/// `magnetite://sync` link that carries both (docs/spec/sync.md, "The code
/// as a QR code"). Spaces are `%20`, which every platform's link parser reads.
pub fn join_link(server: &str, words: &str) -> String {
    let mut link = url::Url::parse("magnetite://sync").expect("a constant link");
    link.query_pairs_mut()
        .append_pair("server", server)
        .append_pair("words", words);
    // A `+` of the input is written as `%2B`, so every `+` left is a space.
    link.as_str().replace('+', "%20")
}

/// `text` as a QR code: one string per row, `1` for a dark square. `None`
/// when the text is too long for one.
pub fn qr_rows(text: &str) -> Option<Vec<String>> {
    let code = qrcode::QrCode::new(text).ok()?;
    let row = |squares: &[qrcode::Color]| {
        squares
            .iter()
            .map(|c| if *c == qrcode::Color::Dark { '1' } else { '0' })
            .collect()
    };
    Some(code.to_colors().chunks(code.width()).map(row).collect())
}

/// The address requests go to (no trailing slash). The server must use
/// `https://`; plain `http://` is accepted only for this machine (`localhost`,
/// `127.0.0.1`, `::1`). A path prefix is fine; user info and queries are not.
fn server_base(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/');
    let bad = || t!("app.sync.bad_address", url = url);
    let uri: ureq::http::Uri = url.parse().map_err(|_| bad())?;
    let authority = uri.authority().ok_or_else(bad)?;
    if authority.as_str().contains('@') {
        return Err(t!("app.sync.user_name").into());
    }
    if uri.query().is_some() {
        return Err(bad());
    }
    let loopback = matches!(authority.host(), "localhost" | "127.0.0.1" | "[::1]");
    match uri.scheme_str() {
        Some("https") => Ok(url.to_string()),
        Some("http") if loopback => Ok(url.to_string()),
        _ => Err(t!("app.sync.https_only", url = url)),
    }
}

/// Settings minus device-specific fields; what the `settings` collection
/// holds. The search engine is not among them: another device holding the
/// words could otherwise send every search here to an address of its choice.
/// `shortcuts` is left out while no shortcut has other keys, so a version
/// from before them writes the same bytes; such a version writes without
/// them, which reads as "all on their default keys" here. The same goes
/// for `shortcut_priority` and `shortcut_sites`, not for `archive_days`.
#[derive(Serialize, Deserialize)]
struct SyncedSettings {
    auto_archive_hours: u32,
    unload_minutes: u32,
    external_links: String,
    #[serde(default, skip_serializing_if = "magnetite_engine::Overrides::is_empty")]
    shortcuts: magnetite_engine::Overrides,
    #[serde(
        default,
        skip_serializing_if = "magnetite_engine::Priorities::is_empty"
    )]
    shortcut_priority: magnetite_engine::Priorities,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    shortcut_sites: Vec<String>,
    /// How long archived tabs are kept. Left out while it is the usual
    /// period and the chain's copy has none, so a version from before
    /// writes the same bytes; sent always once the chain's copy has it
    /// (`period_carried`), so going back to the usual one reaches the
    /// others. Missing, as a version from before writes, this device's
    /// stays: read as the usual period it would delete older records for
    /// good.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    archive_days: Option<u32>,
}

impl SyncedSettings {
    fn from(s: &Settings) -> Self {
        Self {
            auto_archive_hours: s.auto_archive_hours,
            unload_minutes: s.unload_minutes,
            external_links: s.external_links.clone(),
            shortcuts: s.shortcuts.clone(),
            shortcut_priority: s.shortcut_priority.clone(),
            shortcut_sites: s.shortcut_sites.clone(),
            archive_days: (s.archive_days != Settings::default().archive_days)
                .then_some(s.archive_days),
        }
    }
    fn apply(self, s: &mut Settings) {
        s.auto_archive_hours = self.auto_archive_hours;
        s.unload_minutes = self.unload_minutes;
        s.external_links = self.external_links;
        s.shortcuts = self.shortcuts;
        s.shortcut_priority = self.shortcut_priority;
        s.shortcut_sites = self.shortcut_sites;
        if let Some(days) = self.archive_days {
            s.archive_days = days;
        }
    }
}

/// Work items for the sync thread.
pub enum Job {
    /// Start a new sync chain on `server` (a fresh sync code).
    Create {
        server: String,
        invite: Option<String>,
    },
    /// Join an existing chain with its 24 words.
    Join { server: String, words: String },
    /// Pull remote changes; write local data the server does not have yet.
    Sync {
        snapshot: Box<SidebarSnapshot>,
        settings: Box<Settings>,
        /// The sidebar is as this device last took it from sync (apart from
        /// what a device always keeps of its own): there is nothing to write,
        /// even where it differs from the server's.
        sidebar_unchanged: bool,
        /// This device among the chain's devices (`devices` module).
        me: Me,
        /// What of the archive syncs (`AppState::archive_for_sync`), and
        /// whether that is what the device and the chain last agreed on.
        archive: Vec<ArchivedTab>,
        archive_unchanged: bool,
    },
    /// Stop syncing on this device; the chain stays on the server.
    Leave,
    /// Delete the chain and all its data from `server`, then leave.
    DeleteChain { server: String },
}

/// Results delivered on the UI thread.
pub enum Outcome {
    Connected {
        server: String,
        /// This device started the chain (it did not join one).
        created: bool,
    },
    /// Starting or joining a chain did not work.
    ConnectFailed(String),
    /// Remote had newer data; the app should replace its own and re-present.
    /// `agreed` as in `Pushed`: the sidebar can go through while the
    /// settings arrive.
    Pulled {
        sidebar: Option<SidebarSnapshot>,
        settings: Option<SyncedSettingsPublic>,
        versions: Vec<(String, u64)>,
        agreed: Option<Box<SidebarSnapshot>>,
    },
    /// Pushed successfully (or nothing to push); remember the new versions.
    /// `agreed`: the sidebar of this round, when the server now holds
    /// exactly it (written, or there already): what this device and the
    /// chain agree on, which a later merge starts from.
    Pushed {
        versions: Vec<(String, u64)>,
        agreed: Option<Box<SidebarSnapshot>>,
    },
    /// The chain's archive. `pulled`: it is newer than this device's, to be
    /// merged with it. `agreed`: the server holds exactly this round's
    /// records (written, or there already). `version`: to remember.
    Archive {
        pulled: Option<Box<Archive>>,
        agreed: Option<Vec<archive::Key>>,
        version: Option<u64>,
    },
    /// Another device wrote while this one was about to: the round is run
    /// again, which takes what that device wrote first.
    Behind,
    /// The tabs open on the chain's other devices, those sent here that
    /// this device took with this round (`sent`: to open now), and which of
    /// the tabs it took over no device lists any more (`forgotten`): their
    /// handover is done. The same for phones' changes to the sidebar: those
    /// no device has made yet (`edits`: to make now, and to note in
    /// `applied`), and those made here that no device lists any more
    /// (`unlisted`). `version`: the `devices` collection's, to remember.
    Devices {
        tabs: Vec<RemoteTab>,
        sent: Vec<RemoteTab>,
        forgotten: Vec<String>,
        edits: Vec<Edit>,
        unlisted: Vec<String>,
        version: u64,
    },
    Error(String),
    Disconnected,
}

/// `SyncedSettings` re-exported without exposing the private type.
pub struct SyncedSettingsPublic(SyncedSettings);
impl SyncedSettingsPublic {
    /// Take another device's settings, checked like the Settings page's.
    pub fn apply(self, s: &mut Settings) {
        self.0.apply(s);
        *s = std::mem::take(s).sanitised();
    }
}

pub type Deliver = Arc<dyn Fn(Outcome) + Send + Sync>;
/// Takes outcomes on whichever thread has them (`Deliver` runs on the UI thread).
type Notify = Arc<dyn Fn(Outcome) + Send + Sync>;

/// What the sync thread is woken up for.
enum Msg {
    Create {
        server: String,
        invite: Option<String>,
    },
    Join {
        server: String,
        words: String,
    },
    /// Run the sync waiting in `Shared::pending`.
    Sync,
    /// Erase the chain on `server`; this device has left it already.
    DeleteRemote {
        server: String,
        code: SyncCode,
    },
}

/// A queued `Job::Sync`.
struct SyncRequest {
    snapshot: Box<SidebarSnapshot>,
    settings: Box<Settings>,
    sidebar_unchanged: bool,
    me: Me,
    archive: Vec<ArchivedTab>,
    archive_unchanged: bool,
}

/// Shared by the UI thread and the sync thread.
struct Shared {
    /// Bumped when this device leaves its chain. The worker hands out results
    /// only while it holds this lock and the epoch is the one its work started
    /// in, so nothing from the old chain arrives after `Disconnected`.
    epoch: Mutex<u64>,
    /// The newest sync request. Each carries the full state, so a newer one
    /// replaces a queued one and at most one waits, however long the server
    /// takes.
    pending: Mutex<Option<SyncRequest>>,
}

pub struct SyncClient {
    tx: Sender<Msg>,
    shared: Arc<Shared>,
    data_dir: PathBuf,
    notify: Notify,
}

impl SyncClient {
    /// Start the worker. `deliver` is called on the UI thread for each outcome.
    pub fn start(data_dir: PathBuf, sync_settings: SyncSettings, deliver: Deliver) -> Self {
        Self::spawn(
            data_dir,
            sync_settings,
            Arc::new(move |o| {
                let d = deliver.clone();
                Engine::post(Box::new(move || d(o)));
            }),
        )
    }

    fn spawn(data_dir: PathBuf, sync_settings: SyncSettings, notify: Notify) -> Self {
        let (tx, rx) = channel::<Msg>();
        let shared = Arc::new(Shared {
            epoch: Mutex::new(0),
            pending: Mutex::new(None),
        });
        let worker = Worker::new(data_dir.clone(), sync_settings);
        // A deletion the server never confirmed is tried again at start.
        if let Some((server, code)) = load_pending_delete(&data_dir) {
            tx.send(Msg::DeleteRemote { server, code }).ok();
        }
        let (s, n) = (shared.clone(), notify.clone());
        thread::Builder::new()
            .name("magnetite-sync".into())
            .spawn(move || run(worker, &rx, &s, &n))
            .expect("spawn sync thread");
        Self {
            tx,
            shared,
            data_dir,
            notify,
        }
    }

    /// Hand `job` to the sync thread. Leaving (and deleting the chain) takes
    /// effect here and now, without waiting for network work in progress;
    /// the server is told afterwards, best effort.
    pub fn submit(&self, job: Job) {
        let msg = match job {
            Job::Create { server, invite } => Msg::Create { server, invite },
            Job::Join { server, words } => Msg::Join { server, words },
            Job::Sync {
                snapshot,
                settings,
                sidebar_unchanged,
                me,
                archive,
                archive_unchanged,
            } => {
                let request = SyncRequest {
                    snapshot,
                    settings,
                    sidebar_unchanged,
                    me,
                    archive,
                    archive_unchanged,
                };
                if lock(&self.shared.pending).replace(request).is_some() {
                    return; // the wake-up queued for the older one runs this
                }
                Msg::Sync
            }
            Job::Leave => {
                self.leave();
                return;
            }
            Job::DeleteChain { server } => {
                let code = load_code(&self.data_dir);
                // Kept until the server confirms, so a failed deletion can be
                // tried again even though the device leaves the chain now.
                if let Some(code) = &code
                    && let Err(e) = save_pending_delete(&self.data_dir, &server, code)
                {
                    (self.notify)(Outcome::Error(t!("app.sync.remember_deletion", error = e)));
                }
                self.leave();
                let Some(code) = code else { return };
                Msg::DeleteRemote { server, code }
            }
        };
        self.tx.send(msg).ok();
    }

    fn leave(&self) {
        let mut epoch = lock(&self.shared.epoch);
        *epoch += 1;
        lock(&self.shared.pending).take();
        forget_code(&self.data_dir);
        (self.notify)(Outcome::Disconnected);
    }
}

/// The sync thread: one message at a time, results to `notify`.
fn run(mut w: Worker, rx: &Receiver<Msg>, shared: &Shared, notify: &Notify) {
    let mut seen = 0;
    for msg in rx {
        let epoch = *lock(&shared.epoch);
        if epoch != seen {
            // The device left its chain (and may have joined another) since.
            w.use_code(load_code(&w.data_dir));
            seen = epoch;
        }
        let outcomes = match msg {
            Msg::Create { server, invite } => connect_outcomes(w.create(&server, invite)),
            Msg::Join { server, words } => connect_outcomes(w.join(&server, &words)),
            Msg::Sync => {
                // Not inside the `match`: its lock would last the whole sync.
                let request = lock(&shared.pending).take();
                request.map_or_else(Vec::new, |r| {
                    let mut outcomes = w.sync(&r.snapshot, &r.settings, r.sidebar_unchanged);
                    outcomes.extend(w.archive(&r.archive, r.archive_unchanged));
                    outcomes.extend(w.devices(&r.me));
                    outcomes
                })
            }
            Msg::DeleteRemote { server, code } => w.delete_remote(&server, &code),
        };
        let current = lock(&shared.epoch);
        if *current != epoch {
            // Left while this ran: its results no longer apply, and a code
            // it just adopted is forgotten again.
            forget_code(&w.data_dir);
            continue;
        }
        for o in outcomes {
            notify(o);
        }
    }
}

/// What a start or join ended with, told apart from the errors of a round.
fn connect_outcomes(outcomes: Vec<Outcome>) -> Vec<Outcome> {
    outcomes
        .into_iter()
        .map(|o| match o {
            Outcome::Error(e) => Outcome::ConnectFailed(e),
            o => o,
        })
        .collect()
}

struct Worker {
    data_dir: PathBuf,
    settings: SyncSettings,
    code: Option<SyncCode>,
    agent: ureq::Agent,
    /// Highest version per collection this device took from the server or
    /// had acknowledged, in this chain. A job may carry older versions (the
    /// UI thread applies outcomes later); the higher one counts.
    known: HashMap<String, u64>,
}

impl Worker {
    fn new(data_dir: PathBuf, settings: SyncSettings) -> Self {
        let agent = ureq::Agent::config_builder()
            // Status codes are handled here, not turned into errors by ureq.
            .http_status_as_error(false)
            // A redirect could lead to plain http or another host.
            .max_redirects(0)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(REQUEST_TIMEOUT))
            .build()
            .into();
        Self {
            code: load_code(&data_dir),
            data_dir,
            settings,
            agent,
            known: HashMap::new(),
        }
    }

    /// Switch to `code`'s chain (or none): versions start over.
    fn use_code(&mut self, code: Option<SyncCode>) {
        self.code = code;
        self.settings.versions.clear();
        self.known.clear();
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0))
}

/// Why a write did not go through.
enum Unwritten {
    /// Another device wrote first: read again, then write.
    Behind,
    /// What went wrong, for Settings.
    Failed(String),
}

/// A response: status and body bytes.
struct Reply {
    status: u16,
    body: Vec<u8>,
}

impl Reply {
    /// The body as `T`. A parse error would quote the body, so it is not
    /// shown: no text of the server's reaches Settings.
    fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, String> {
        serde_json::from_slice(&self.body).map_err(|_| t!("app.sync.unreadable_answer").to_string())
    }
    /// What went wrong, in our words: the server's own text is never shown,
    /// so a server cannot use Settings to ask for the sync code.
    fn error(&self) -> String {
        match self.status {
            400 => t!("app.sync.status_400").into(),
            401 => t!("app.sync.status_401").into(),
            403 => t!("app.sync.status_403").into(),
            404 => t!("app.sync.status_404").into(),
            408 => t!("app.sync.status_408").into(),
            410 => t!("app.sync.status_410").into(),
            409 => t!("app.sync.status_409").into(),
            413 => t!("app.sync.status_413").into(),
            429 | 503 => t!("app.sync.busy").into(),
            s if s >= 500 => t!("app.sync.server_error").into(),
            s => t!("app.sync.status", status = s),
        }
    }
}

impl Worker {
    /// Signed request to `path` below `/v2` on this device's server.
    fn request(
        &self,
        keys: &ChainKeys,
        method: &str,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<Reply, String> {
        self.request_to(&self.settings.server_url, keys, method, path, body)
    }

    /// Signed request to `path` below `/v2` on `server`. The signature covers
    /// the server's host, so no other server can replay it.
    fn request_to(
        &self,
        server: &str,
        keys: &ChainKeys,
        method: &str,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<Reply, String> {
        let base = server_base(server)?;
        let url = format!("{base}/v2{path}");
        let host = base
            .parse::<ureq::http::Uri>()
            .ok()
            .and_then(|u| u.authority().map(|a| a.as_str().to_string()))
            .unwrap_or_default();
        let bytes = body.unwrap_or_default();
        let builder = ureq::http::Request::builder()
            .method(method)
            .uri(&url)
            .header(
                "authorization",
                keys.authorization(method, &host, path, &bytes, now_ms()),
            )
            // The body's hash ahead of the body: the server checks the
            // signature before it reads an upload.
            .header(
                magnetite_sync_proto::CONTENT_DIGEST,
                magnetite_sync_proto::content_digest(&bytes),
            )
            .header("content-type", "application/json");
        let result = if bytes.is_empty() {
            builder
                .body(())
                .map_err(|e| e.to_string())
                .and_then(|r| self.agent.run(r).map_err(|e| e.to_string()))
        } else {
            builder
                .body(bytes)
                .map_err(|e| e.to_string())
                .and_then(|r| self.agent.run(r).map_err(|e| e.to_string()))
        };
        let mut res = result.map_err(|e| t!("app.sync.unreachable", error = e))?;
        let status = res.status().as_u16();
        // ureq applies the limit to the decoded body (since 3.5.2), so a
        // compressed answer counts unpacked.
        let body = res
            .body_mut()
            .with_config()
            .limit(MAX_REPLY)
            .read_to_vec()
            .map_err(|e| match e {
                ureq::Error::BodyExceedsLimit(_) => t!("app.sync.too_much").to_string(),
                e => e.to_string(),
            })?;
        Ok(Reply { status, body })
    }

    fn adopt(&mut self, server: &str, code: SyncCode, created: bool) -> Vec<Outcome> {
        // A deletion of this very chain that the server never confirmed is
        // off: it would erase the chain the device is in again.
        let this = code.to_base64();
        if load_pending_delete(&self.data_dir).is_some_and(|(_, c)| c.to_base64() == this) {
            std::fs::remove_file(pending_delete_path(&self.data_dir)).ok();
        }
        if let Err(e) = save_code(&self.data_dir, &code) {
            return vec![Outcome::Error(t!("app.sync.store_code", error = e))];
        }
        self.use_code(Some(code));
        self.settings.started_here = created;
        vec![Outcome::Connected {
            server: server.to_string(),
            created,
        }]
    }

    /// A second chain over the first would replace its sync code, and the
    /// first chain could then neither be used nor deleted.
    fn in_a_chain(&self) -> Option<Vec<Outcome>> {
        self.code
            .as_ref()
            .map(|_| vec![Outcome::Error(t!("app.sync.already_in_chain").into())])
    }

    fn create(&mut self, server: &str, invite: Option<String>) -> Vec<Outcome> {
        if let Some(refused) = self.in_a_chain() {
            return refused;
        }
        self.settings.server_url = server.trim().to_string();
        let code = SyncCode::generate();
        let body = serde_json::to_vec(&CreateChainRequest {
            invite: invite.filter(|i| !i.trim().is_empty()),
        })
        .unwrap_or_default();
        match self.request(&code.keys(), "POST", "/chain", Some(body)) {
            Ok(r) if r.status == 201 || r.status == 204 => self.adopt(server.trim(), code, true),
            Ok(r) => vec![Outcome::Error(r.error())],
            Err(e) => vec![Outcome::Error(e)],
        }
    }

    fn join(&mut self, server: &str, words: &str) -> Vec<Outcome> {
        if let Some(refused) = self.in_a_chain() {
            return refused;
        }
        self.settings.server_url = server.trim().to_string();
        let code = match SyncCode::from_words(words) {
            Ok(c) => c,
            Err(e) => return vec![Outcome::Error(e.to_string())],
        };
        match self.request(&code.keys(), "GET", "/chain", None) {
            Ok(r) if r.status == 204 => self.adopt(server.trim(), code, false),
            // The server tells only the key holder that a chain does not exist.
            Ok(r) if r.status == 404 => vec![Outcome::Error(t!(
                "app.sync.no_chain",
                server = self.settings.server_url
            ))],
            Ok(r) => vec![Outcome::Error(r.error())],
            Err(e) => vec![Outcome::Error(e)],
        }
    }

    fn sync(
        &mut self,
        snapshot: &SidebarSnapshot,
        settings: &Settings,
        sidebar_unchanged: bool,
    ) -> Vec<Outcome> {
        let Some(code) = self.code.clone() else {
            return vec![Outcome::Error(t!("app.sync.not_in_chain").into())];
        };
        let keys = code.keys();
        self.settings = settings.sync.clone();
        // What went wrong per collection, reported after what went through.
        let mut errors: Vec<String> = Vec::new();
        let mut pulled_sidebar = None;
        let mut pulled_settings = None;
        let mut versions = Vec::new();
        // The server holds exactly this round's sidebar.
        let mut held = false;
        let mut behind = false;
        let mut plain: [(&str, Vec<u8>); 2] = [
            (SIDEBAR, serde_json::to_vec(snapshot).unwrap_or_default()),
            (
                SETTINGS,
                serde_json::to_vec(&SyncedSettings::from(settings)).unwrap_or_default(),
            ),
        ];
        for (name, bytes) in &mut plain {
            let id = keys.collection_id(name);
            let path = format!("/collections/{id}");
            // The version this job's snapshot is built on, and the newest this
            // device has seen (a pull the UI thread may not have applied yet).
            let applied = self.settings.versions.get(*name).copied().unwrap_or(0);
            let seen = applied.max(self.known.get(*name).copied().unwrap_or(0));
            // Whether the server holds exactly what this device would write.
            let mut same = false;
            // Pull first: if the server is ahead, take it.
            match self.request(&keys, "GET", &path, None) {
                Ok(r) if r.status == 200 => {
                    let remote: CollectionResponse = match r.json() {
                        Ok(c) => c,
                        Err(e) => {
                            errors.push(e);
                            continue;
                        }
                    };
                    if remote.version < seen {
                        errors.push(t!(
                            "app.sync.going_back",
                            name = name,
                            remote = remote.version,
                            seen = seen
                        ));
                        continue;
                    }
                    if remote.version > seen {
                        let read = keys
                            .open(&id, remote.version, &remote.sealed())
                            .map_err(|e| e.to_string())
                            .and_then(|data| {
                                // Data this version cannot read (a newer Magnetite
                                // wrote it) is not taken: counting it as applied
                                // would let this device overwrite it later.
                                let unreadable = t!("app.sync.unreadable");
                                if *name == SIDEBAR {
                                    // One without spaces cannot be shown. Not
                                    // counted as seen: every round says so
                                    // until a usable one is there.
                                    match serde_json::from_slice::<SidebarSnapshot>(&data) {
                                        Ok(s) if s.spaces.is_empty() => {
                                            Err(t!("app.sync.no_spaces").to_string())
                                        }
                                        // A format this version must not
                                        // write over: it would write it back
                                        // as its own, older one.
                                        Ok(s) if s.version > SidebarSnapshot::VERSION => {
                                            Err(t!("app.sync.newer_format").to_string())
                                        }
                                        Ok(s) => {
                                            pulled_sidebar = Some(s);
                                            Ok(())
                                        }
                                        Err(_) => Err(unreadable.to_string()),
                                    }
                                } else {
                                    serde_json::from_slice::<SyncedSettings>(&data)
                                        .map(|s| pulled_settings = Some(SyncedSettingsPublic(s)))
                                        .map_err(|_| unreadable.to_string())
                                }
                            });
                        match read {
                            Ok(()) => {
                                self.known.insert((*name).to_string(), remote.version);
                                versions.push(((*name).to_string(), remote.version));
                            }
                            Err(e) => {
                                errors.push(t!(
                                    "app.sync.version_failed",
                                    name = name,
                                    version = remote.version,
                                    error = e
                                ));
                            }
                        }
                        continue;
                    }
                    // Compared by content, not by a count of local changes:
                    // the app's state changes all the time (a page's title,
                    // a download) without the synced data changing. Writing
                    // the same data again costs a request and a version, and
                    // every other device would take it, or merge its own
                    // changes into it, for nothing.
                    let data = keys.open(&id, remote.version, &remote.sealed()).ok();
                    // What the chain holds in profiles private here stays
                    // as it is there (`carried`), and so does that the
                    // settings name the archive's period (`period_carried`).
                    let sent = data.as_deref().and_then(|d| match *name {
                        SIDEBAR => carried(snapshot, d),
                        _ => period_carried(settings, d),
                    });
                    if let Some(sent) = sent {
                        *bytes = sent;
                    }
                    same = data.is_some_and(|data| data == *bytes);
                    held |= same && *name == SIDEBAR;
                }
                // Nothing there yet. Only the device that started the chain
                // writes first: a device that joined before that one's first
                // write went through would make its own sidebar the chain's.
                Ok(r) if r.status == 404 => {
                    if applied == 0 && !self.settings.started_here {
                        errors.push(t!("app.sync.no_data_yet", name = name));
                        continue;
                    }
                }
                Ok(r) => {
                    errors.push(r.error());
                    continue;
                }
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            }
            if seen > applied {
                // This snapshot predates a pull the UI thread is applying; it
                // queues a fresh one then. Writing this one would undo the pull.
                continue;
            }
            // A sidebar without spaces is not written: no device would take
            // it. A device has one when it keeps all its spaces to itself.
            let nothing_to_send = *name == SIDEBAR && snapshot.spaces.is_empty();
            if same || nothing_to_send || (*name == SIDEBAR && sidebar_unchanged) {
                continue;
            }
            match self.push(&keys, name, applied, bytes) {
                Ok(version) => {
                    self.known.insert((*name).to_string(), version);
                    versions.push(((*name).to_string(), version));
                    held |= *name == SIDEBAR;
                }
                Err(Unwritten::Behind) => {
                    errors.push(t!("app.sync.wrote_first", name = name));
                    behind = true;
                }
                Err(Unwritten::Failed(e)) => errors.push(e),
            }
        }
        let mut outcomes = Vec::new();
        let agreed = held.then(|| Box::new(snapshot.clone()));
        if pulled_sidebar.is_some() || pulled_settings.is_some() {
            outcomes.push(Outcome::Pulled {
                sidebar: pulled_sidebar,
                settings: pulled_settings,
                versions,
                agreed,
            });
        } else if errors.is_empty() || !versions.is_empty() || agreed.is_some() {
            // Also when only the sidebar went through, there already: a
            // later merge starts from it.
            outcomes.push(Outcome::Pushed { versions, agreed });
        }
        // Last: a collection that went through must not hide the other's
        // failure.
        outcomes.extend(errors.into_iter().map(Outcome::Error));
        if behind {
            outcomes.push(Outcome::Behind);
        }
        outcomes
    }

    /// Write collection `name` as the version after `base`. Returns that
    /// version once the server acknowledged exactly it.
    fn push(
        &self,
        keys: &ChainKeys,
        name: &str,
        base: u64,
        bytes: &[u8],
    ) -> Result<u64, Unwritten> {
        let failed = Unwritten::Failed;
        let id = keys.collection_id(name);
        let next = base
            .checked_add(1)
            .ok_or_else(|| failed(t!("app.sync.overflow", name = name)))?;
        let sealed = keys
            .seal(&id, next, bytes)
            .map_err(|e| failed(e.to_string()))?;
        let body = serde_json::to_vec(&PutCollectionRequest::new(base, sealed)).unwrap_or_default();
        let r = (self.request(keys, "PUT", &format!("/collections/{id}"), Some(body)))
            .map_err(failed)?;
        match r.status {
            200 => match r.json::<PutCollectionResponse>().map_err(failed)?.version {
                v if v == next => Ok(next),
                v => Err(failed(t!(
                    "app.sync.wrong_version",
                    name = name,
                    acked = v,
                    next = next
                ))),
            },
            409 => Err(Unwritten::Behind),
            _ => Err(failed(r.error())),
        }
    }

    /// The archive's round: take the chain's if it is newer, else write
    /// this device's records (`mine`) if they changed (`unchanged`: they
    /// are what the device and the chain last agreed on). Any device may
    /// write first, unlike with the sidebar: archives are merged, never
    /// replaced. Versions only go up, as in `sync`.
    fn archive(&mut self, mine: &[ArchivedTab], unchanged: bool) -> Vec<Outcome> {
        let Some(code) = self.code.clone() else {
            return Vec::new();
        };
        let keys = code.keys();
        let id = keys.collection_id(ARCHIVE);
        let path = format!("/collections/{id}");
        let applied = self.settings.versions.get(ARCHIVE).copied().unwrap_or(0);
        let seen = applied.max(self.known.get(ARCHIVE).copied().unwrap_or(0));
        let failed = |error: String, version: u64| {
            vec![Outcome::Error(t!(
                "app.sync.version_failed",
                name = ARCHIVE,
                version = version,
                error = error
            ))]
        };
        let agreed = |version| {
            vec![Outcome::Archive {
                pulled: None,
                agreed: Some(archive::keys(mine)),
                version,
            }]
        };
        // The chain's copy: a write keeps what it says besides the records
        // (`Archive::to_write`).
        let chain = match self.request(&keys, "GET", &path, None) {
            Ok(r) if r.status == 200 => {
                let remote: CollectionResponse = match r.json() {
                    Ok(c) => c,
                    Err(e) => return vec![Outcome::Error(e)],
                };
                if remote.version < seen {
                    return vec![Outcome::Error(t!(
                        "app.sync.going_back",
                        name = ARCHIVE,
                        remote = remote.version,
                        seen = seen
                    ))];
                }
                // Not counted as seen when it cannot be read, as in `sync`:
                // this device then never writes over it.
                let chain = match keys
                    .open(&id, remote.version, &remote.sealed())
                    .ok()
                    .and_then(|data| serde_json::from_slice::<Archive>(&data).ok())
                {
                    Some(a) if a.version > Archive::VERSION => {
                        return failed(t!("app.sync.newer_format").into(), remote.version);
                    }
                    Some(a) => a,
                    None => return failed(t!("app.sync.unreadable").into(), remote.version),
                };
                if remote.version > seen {
                    self.known.insert(ARCHIVE.to_string(), remote.version);
                    return vec![Outcome::Archive {
                        pulled: Some(Box::new(chain)),
                        agreed: None,
                        version: Some(remote.version),
                    }];
                }
                Some(chain)
            }
            // Nothing there yet, and nothing to put there.
            Ok(r) if r.status == 404 && seen == 0 => {
                if mine.is_empty() {
                    return Vec::new();
                }
                None
            }
            Ok(r) if r.status == 404 => {
                return vec![Outcome::Error(t!(
                    "app.sync.going_back",
                    name = ARCHIVE,
                    remote = 0,
                    seen = seen
                ))];
            }
            Ok(r) => return vec![Outcome::Error(r.error())],
            Err(e) => return vec![Outcome::Error(e)],
        };
        if chain.as_ref().is_some_and(|c| c.entries == mine) {
            return agreed(None);
        }
        // `seen > applied`: these records predate a pull the UI thread is
        // applying; writing them would undo it.
        if unchanged || seen > applied {
            return Vec::new();
        }
        let bytes =
            serde_json::to_vec(&Archive::to_write(chain, mine.to_vec())).unwrap_or_default();
        match self.push(&keys, ARCHIVE, applied, &bytes) {
            Ok(version) => {
                self.known.insert(ARCHIVE.to_string(), version);
                agreed(Some(version))
            }
            Err(Unwritten::Behind) => vec![
                Outcome::Error(t!("app.sync.wrote_first", name = ARCHIVE)),
                Outcome::Behind,
            ],
            Err(Unwritten::Failed(e)) => vec![Outcome::Error(e)],
        }
    }

    /// Read the tabs open on the chain's other devices, and write which of
    /// them this device took over. Only this device's entry changes; when
    /// another device wrote in between, the round starts over. A version
    /// older than one this device has seen is refused, as in `sync`: an old
    /// copy could list a tab again whose handover is done.
    fn devices(&mut self, me: &Me) -> Vec<Outcome> {
        let Some(code) = self.code.clone() else {
            return Vec::new();
        };
        let keys = code.keys();
        let id = keys.collection_id(DEVICES);
        let path = format!("/collections/{id}");
        let unreadable = || t!("app.sync.devices_unreadable").to_string();
        let applied = self.settings.versions.get(DEVICES).copied().unwrap_or(0);
        for _ in 0..4 {
            let seen = applied.max(self.known.get(DEVICES).copied().unwrap_or(0));
            // The server's version is older than one seen, or it has none.
            let back = |remote: Option<u64>| {
                vec![Outcome::Error(match remote {
                    Some(remote) => t!("app.sync.devices_going_back", remote = remote, seen = seen),
                    None => t!("app.sync.devices_gone", seen = seen),
                })]
            };
            let (version, mut all) = match self.request(&keys, "GET", &path, None) {
                Ok(r) if r.status == 200 => {
                    let remote: CollectionResponse = match r.json() {
                        Ok(c) => c,
                        Err(e) => return vec![Outcome::Error(e)],
                    };
                    if remote.version < seen {
                        return back(Some(remote.version));
                    }
                    // Data this version cannot read is not written over.
                    let read = keys
                        .open(&id, remote.version, &remote.sealed())
                        .ok()
                        .and_then(|data| serde_json::from_slice::<Devices>(&data).ok());
                    match read {
                        Some(all) => (remote.version, all),
                        None => return vec![Outcome::Error(unreadable())],
                    }
                }
                // No device listed a tab yet.
                Ok(r) if r.status == 404 && seen == 0 => (0, Devices::default()),
                Ok(r) if r.status == 404 => return back(None),
                Ok(r) => return vec![Outcome::Error(r.error())],
                Err(e) => return vec![Outcome::Error(e)],
            };
            self.known.insert(DEVICES.to_string(), version);
            let round = devices::round(&mut all, me, crate::state::now());
            let done = |version| {
                vec![Outcome::Devices {
                    tabs: round.tabs,
                    sent: round.sent,
                    forgotten: me
                        .taken
                        .iter()
                        .filter(|id| !round.taken.contains(id))
                        .cloned()
                        .collect(),
                    edits: round.edits,
                    unlisted: me
                        .applied
                        .iter()
                        .filter(|id| !round.applied.contains(id))
                        .cloned()
                        .collect(),
                    version,
                }]
            };
            if !round.write {
                return done(version);
            }
            let bytes = serde_json::to_vec(&all).unwrap_or_default();
            match self.push(&keys, DEVICES, version, &bytes) {
                Ok(written) => {
                    self.known.insert(DEVICES.to_string(), written);
                    return done(written);
                }
                // Another device wrote first: read again.
                Err(Unwritten::Behind) => {}
                Err(Unwritten::Failed(e)) => return vec![Outcome::Error(e)],
            }
        }
        vec![Outcome::Error(t!("app.sync.devices_busy").into())]
    }

    /// Erase `code`'s chain on `server`. This device left it already, so only
    /// a failure is reported; the pending record stays for another try.
    fn delete_remote(&self, server: &str, code: &SyncCode) -> Vec<Outcome> {
        let failed = |e: String| vec![Outcome::Error(t!("app.sync.delete_pending", error = e))];
        // 404: the chain is gone already. Anything else, also a refused
        // signature (401, for example after a wrong clock), is no proof.
        match self.request_to(server, &code.keys(), "DELETE", "/chain", None) {
            Ok(r) if r.status == 204 || r.status == 404 => {
                std::fs::remove_file(pending_delete_path(&self.data_dir)).ok();
                Vec::new()
            }
            Ok(r) => failed(r.error()),
            Err(e) => failed(e),
        }
    }
}

/// A chain deletion the server has not confirmed yet waits to be tried again.
pub fn deletion_pending(data_dir: &Path) -> bool {
    pending_delete_path(data_dir).exists()
}

pub fn signed_in(data_dir: &Path) -> bool {
    load_code(data_dir).is_some()
}

/// `snapshot` as it is written over the chain's sidebar `chain` (its
/// plain bytes): with what the chain holds in profiles that are private on
/// this device (`carry_private_tabs`). None when that changes nothing.
fn carried(snapshot: &SidebarSnapshot, chain: &[u8]) -> Option<Vec<u8>> {
    if !snapshot.profiles.iter().any(|p| p.ephemeral) {
        return None;
    }
    let chain: SidebarSnapshot = serde_json::from_slice(chain).ok()?;
    let mut sent = snapshot.clone();
    crate::state::carry_private_tabs(&mut sent, &chain);
    (sent != *snapshot)
        .then(|| serde_json::to_vec(&sent).ok())
        .flatten()
}

/// The settings as sent when the chain's copy (`chain`) names how long
/// archived tabs are kept and this device's period is the usual one: with
/// it, so a device that went back to it does not keep the old one there.
/// None: as `SyncedSettings::from` builds them.
fn period_carried(settings: &Settings, chain: &[u8]) -> Option<Vec<u8>> {
    let chain: SyncedSettings = serde_json::from_slice(chain).ok()?;
    let mut sent = SyncedSettings::from(settings);
    if chain.archive_days.is_none() || sent.archive_days.is_some() {
        return None;
    }
    sent.archive_days = Some(settings.archive_days);
    serde_json::to_vec(&sent).ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn shortcuts_are_sent_only_once_one_has_other_keys() {
        let mut settings = Settings::default();
        let sent = |s: &Settings| serde_json::to_string(&super::SyncedSettings::from(s)).unwrap();
        // As a version from before changeable shortcuts writes them.
        assert!(!sent(&settings).contains("shortcuts"));
        let keys = "primary+K".parse().unwrap();
        (settings.shortcuts)
            .set(magnetite_engine::Action::NewTab, Some(keys))
            .unwrap();
        assert!(sent(&settings).contains(r#""shortcuts":{"new_tab":"primary+K"}"#));
        // Such a version's settings put every shortcut back on its default keys.
        let older: super::SyncedSettings =
            serde_json::from_str(&sent(&Settings::default())).unwrap();
        older.apply(&mut settings);
        assert!(settings.shortcuts.is_empty());
    }

    #[test]
    fn how_long_archived_tabs_are_kept_is_sent_once_it_is_not_the_usual() {
        let mut settings = Settings::default();
        let sent = |s: &Settings| serde_json::to_string(&super::SyncedSettings::from(s)).unwrap();
        // As a version from before the archive synced writes the settings.
        let older = sent(&settings);
        assert!(!older.contains("archive_days"));
        settings.archive_days = 30;
        assert!(sent(&settings).contains(r#""archive_days":30"#));
        let theirs: super::SyncedSettings = serde_json::from_str(&sent(&settings)).unwrap();
        let mut here = Settings::default();
        theirs.apply(&mut here);
        assert_eq!(here.archive_days, 30);
        // Settings without it keep this device's, "Forever" too: as the
        // usual period they would delete older records for good.
        here.archive_days = 0;
        let older: super::SyncedSettings = serde_json::from_str(&older).unwrap();
        older.apply(&mut here);
        assert_eq!(here.archive_days, 0);
    }

    #[test]
    fn going_back_to_the_usual_archive_period_reaches_the_chain() {
        let usual = Settings::default();
        let longer = Settings {
            archive_days: 365,
            ..Settings::default()
        };
        let chain = |s: &Settings| serde_json::to_vec(&super::SyncedSettings::from(s)).unwrap();
        // The chain's copy names a period: this device's goes along.
        let sent = super::period_carried(&usual, &chain(&longer)).unwrap();
        let sent: super::SyncedSettings = serde_json::from_slice(&sent).unwrap();
        assert_eq!(sent.archive_days, Some(usual.archive_days));
        // It names none, or this device's is sent anyway: as built.
        assert_eq!(super::period_carried(&usual, &chain(&usual)), None);
        assert_eq!(super::period_carried(&longer, &chain(&longer)), None);
    }

    use super::*;

    #[test]
    fn join_link_carries_server_and_words() {
        let words = SyncCode::generate().words();
        let link = join_link("https://sync.example.org/a b+c", &words);
        assert!(link.starts_with(
            "magnetite://sync?server=https%3A%2F%2Fsync.example.org%2Fa%20b%2Bc&words="
        ));
        assert!(!link.contains('+') && !link.contains(' '));
        // Read back by a parser that, like a phone's, takes `+` as itself.
        let read = link.rsplit_once("&words=").unwrap().1.replace("%20", " ");
        assert_eq!(read, words);

        let rows = qr_rows(&link).unwrap();
        assert!(rows.iter().all(|r| r.len() == rows.len()));
        // The finder pattern's corner.
        assert!(rows[0].starts_with("1111111"));
    }
    use magnetite_core::{Profile, SidebarSnapshot, Space};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    fn snapshot(title: &str) -> SidebarSnapshot {
        let mut s = SidebarSnapshot::new();
        s.profiles.push(Profile {
            id: "default".into(),
            name: "Default".into(),
            is_default: true,
            ephemeral: false,
            color: None,
            other: magnetite_core::Other::new(),
        });
        s.spaces.push(Space {
            id: "s1".into(),
            title: title.into(),
            icon: None,
            icon_image: None,
            accent: None,
            profile_id: "default".into(),
            shortcut: None,
            pinned: vec![],
            today: vec![],
            bookmarks: Vec::new(),
            other: magnetite_core::Other::new(),
        });
        s
    }

    fn settings_with(sync: &SyncSettings) -> Settings {
        Settings {
            sync: sync.clone(),
            ..Settings::default()
        }
    }

    fn describe(o: &Outcome) -> String {
        match o {
            Outcome::Connected { .. } => "Connected".into(),
            Outcome::ConnectFailed(e) => format!("ConnectFailed({e})"),
            Outcome::Pulled { .. } => "Pulled".into(),
            Outcome::Pushed { .. } => "Pushed".into(),
            Outcome::Behind => "Behind".into(),
            Outcome::Archive { .. } => "Archive".into(),
            Outcome::Devices { .. } => "Devices".into(),
            Outcome::Error(e) => format!("Error({e})"),
            Outcome::Disconnected => "Disconnected".into(),
        }
    }

    /// A fresh data directory for one test.
    fn data_dir(name: &str) -> PathBuf {
        let dir = crate::files::test_dir(&format!("sync-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A misbehaving server: `answer(method, path)` gives status and JSON
    /// body for every request. Returns its `http://127.0.0.1:<port>` address.
    fn fake_server(answer: impl Fn(&str, &str) -> (u16, String) + Send + 'static) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut words = line.split_whitespace();
                let (method, path) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                }
                reader.read_exact(&mut vec![0; length]).unwrap();
                let (status, body) = answer(method, path);
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        url
    }

    /// A worker in a stored sync chain, talking to `server`.
    fn worker_in_chain(name: &str, server: &str) -> (Worker, SyncCode) {
        let dir = data_dir(name);
        let code = SyncCode::generate();
        save_code(&dir, &code).unwrap();
        let settings = SyncSettings {
            server_url: server.into(),
            // The device that started the chain: it may write first.
            started_here: true,
            ..SyncSettings::default()
        };
        (Worker::new(dir, settings), code)
    }

    fn sync_settings(versions: u64) -> Settings {
        let mut s = Settings::default();
        s.sync.versions = [
            (SIDEBAR.to_string(), versions),
            (SETTINGS.to_string(), versions),
        ]
        .into();
        s
    }

    #[test]
    fn writes_count_only_when_acknowledged_at_the_version_written() {
        let ack = Arc::new(AtomicU64::new(0));
        let answer = ack.clone();
        let server = fake_server(move |method, _| match method {
            "GET" => (404, r#"{"error":"no such collection"}"#.into()),
            _ => (
                200,
                format!(r#"{{"version":{}}}"#, answer.load(Ordering::SeqCst)),
            ),
        });
        let (mut w, _) = worker_in_chain("ack", &server);
        let settings = settings_with(&w.settings);
        // Writes go out as version 1; lower, higher or absurd answers are refused.
        for wrong in [0, 2, u64::MAX] {
            ack.store(wrong, Ordering::SeqCst);
            let out = w.sync(&snapshot("Work"), &settings, false);
            assert!(
                out.iter()
                    .all(|o| matches!(o, Outcome::Error(e) if e.contains("acknowledged"))),
                "ack {wrong}: {:?}",
                out.iter().map(describe).collect::<Vec<_>>()
            );
        }
        ack.store(1, Ordering::SeqCst);
        let out = w.sync(&snapshot("Work"), &settings, false);
        let [Outcome::Pushed { versions, .. }] = out.as_slice() else {
            panic!("{:?}", out.iter().map(describe).collect::<Vec<_>>())
        };
        assert_eq!(versions, &[(SIDEBAR.into(), 1), (SETTINGS.into(), 1)]);
    }

    #[test]
    fn a_round_another_device_wrote_into_first_is_run_again() {
        let server = fake_server(|method, _| match method {
            "GET" => (404, r#"{"error":"no such collection"}"#.into()),
            _ => (409, r#"{"error":"version conflict"}"#.into()),
        });
        let (mut w, _) = worker_in_chain("behind", &server);
        let settings = settings_with(&w.settings);
        let out = w.sync(&snapshot("Work"), &settings, false);
        // Nothing went through, so nothing is agreed on; the app is told
        // to read what the other device wrote.
        assert!(
            matches!(
                out.as_slice(),
                [Outcome::Error(_), Outcome::Error(_), Outcome::Behind]
            ),
            "{:?}",
            out.iter().map(describe).collect::<Vec<_>>()
        );
    }

    fn archived(id: &str, at: i64) -> ArchivedTab {
        ArchivedTab {
            tab: magnetite_core::TabItem::new(id, format!("https://example.org/{id}"), id),
            space_id: "s1".into(),
            archived_at: at,
            other: magnetite_core::Other::new(),
        }
    }

    #[test]
    fn the_archive_is_written_by_whoever_has_one_and_taken_when_newer() {
        // The server holds what was last put, at the version in `held`.
        let held = Arc::new(Mutex::new((0u64, Vec::<ArchivedTab>::new())));
        let code = SyncCode::generate();
        let keys = code.keys();
        let id = keys.collection_id(ARCHIVE);
        let server = {
            let (held, keys, id) = (held.clone(), code.keys(), id.clone());
            fake_server(move |method, _| {
                let (version, entries) = lock(&held).clone();
                if method == "PUT" {
                    return (200, format!(r#"{{"version":{}}}"#, version + 1));
                }
                if version == 0 {
                    return (404, "{}".into());
                }
                let plain = serde_json::to_vec(&Archive::new(entries)).unwrap();
                let body =
                    CollectionResponse::new(version, keys.seal(&id, version, &plain).unwrap());
                (200, serde_json::to_string(&body).unwrap())
            })
        };
        let dir = data_dir("archive");
        save_code(&dir, &code).unwrap();
        // A device that joined: it may not write a sidebar first, but an
        // archive it may.
        let mut w = Worker::new(
            dir,
            SyncSettings {
                server_url: server,
                ..SyncSettings::default()
            },
        );
        // Nothing here, nothing there: not a word.
        assert!(w.archive(&[], false).is_empty());
        let mine = [archived("a", 1), archived("b", 2)];
        let out = w.archive(&mine, false);
        let [
            Outcome::Archive {
                pulled: None,
                agreed: Some(agreed),
                version: Some(1),
            },
        ] = out.as_slice()
        else {
            panic!("{:?}", out.iter().map(describe).collect::<Vec<_>>())
        };
        assert_eq!(*agreed, archive::keys(&mine));

        // Another device wrote: its records are handed over, none written.
        *lock(&held) = (2, vec![archived("b", 2), archived("c", 3)]);
        let out = w.archive(&mine, false);
        let [
            Outcome::Archive {
                pulled: Some(theirs),
                agreed: None,
                version: Some(2),
            },
        ] = out.as_slice()
        else {
            panic!("{:?}", out.iter().map(describe).collect::<Vec<_>>())
        };
        assert_eq!(theirs.entries.len(), 2);
        // The same records as the server's: agreed, without a write.
        w.settings.versions.insert(ARCHIVE.into(), 2);
        let same = [archived("b", 2), archived("c", 3)];
        assert!(matches!(
            w.archive(&same, false).as_slice(),
            [Outcome::Archive {
                pulled: None,
                agreed: Some(_),
                version: None
            }]
        ));
        // An older copy is refused.
        *lock(&held) = (1, Vec::new());
        assert!(
            matches!(w.archive(&same, false).as_slice(), [Outcome::Error(e)] if e.contains("not going back"))
        );
    }

    #[test]
    fn what_the_chain_holds_in_a_profile_private_here_is_not_written_away() {
        // This device's sidebar: a profile that is private here.
        let mut mine = snapshot("Work");
        mine.profiles.push(Profile {
            id: "secret".into(),
            name: "Secret".into(),
            is_default: false,
            ephemeral: true,
            color: None,
            other: magnetite_core::Other::new(),
        });
        // The chain's: the same, with another device's tab in that profile.
        let mut chain = mine.clone();
        chain.spaces[0].today.push(magnetite_core::TabItem {
            profile_id: Some("secret".into()),
            ..magnetite_core::TabItem::new("t", "https://example.org/t", "t")
        });
        let puts = Arc::new(AtomicUsize::new(0));
        let code = SyncCode::generate();
        let server = {
            let keys = code.keys();
            let sidebar = keys.collection_id(SIDEBAR);
            let puts = puts.clone();
            fake_server(move |method, path| {
                if method == "PUT" {
                    puts.fetch_add(1, Ordering::SeqCst);
                    return (200, r#"{"version":4}"#.into());
                }
                let (id, plain) = if path.ends_with(sidebar.as_str()) {
                    (sidebar.clone(), serde_json::to_vec(&chain).unwrap())
                } else {
                    let plain = SyncedSettings::from(&Settings::default());
                    (
                        keys.collection_id(SETTINGS),
                        serde_json::to_vec(&plain).unwrap(),
                    )
                };
                let body = CollectionResponse::new(3, keys.seal(&id, 3, &plain).unwrap());
                (200, serde_json::to_string(&body).unwrap())
            })
        };
        let dir = data_dir("private-here");
        save_code(&dir, &code).unwrap();
        let mut w = Worker::new(dir, SyncSettings::default());
        let mut settings = sync_settings(3);
        settings.sync.server_url = server;
        // Leaving the tab out would close it on the other device: this
        // device writes nothing, and agrees on its own sidebar.
        let out = w.sync(&mine, &settings, false);
        assert!(
            matches!(out.as_slice(), [Outcome::Pushed { versions, agreed: Some(agreed) }]
                if versions.is_empty() && **agreed == mine),
            "{:?}",
            out.iter().map(describe).collect::<Vec<_>>()
        );
        assert_eq!(puts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_sidebar_written_while_the_settings_arrive_is_agreed_on() {
        let code = SyncCode::generate();
        let server = {
            let keys = code.keys();
            let sidebar = keys.collection_id(SIDEBAR);
            fake_server(move |method, path| {
                if method == "PUT" {
                    return (200, r#"{"version":4}"#.into());
                }
                // The sidebar as this device saw it last; settings another
                // device wrote since.
                let (id, version, plain) = if path.ends_with(sidebar.as_str()) {
                    let plain = serde_json::to_vec(&snapshot("Old")).unwrap();
                    (sidebar.clone(), 3, plain)
                } else {
                    let plain = SyncedSettings::from(&Settings::default());
                    let plain = serde_json::to_vec(&plain).unwrap();
                    (keys.collection_id(SETTINGS), 4, plain)
                };
                let sealed = keys.seal(&id, version, &plain).unwrap();
                let body = CollectionResponse::new(version, sealed);
                (200, serde_json::to_string(&body).unwrap())
            })
        };
        let dir = data_dir("written-and-pulled");
        save_code(&dir, &code).unwrap();
        let mut w = Worker::new(dir, SyncSettings::default());
        let mut settings = sync_settings(3);
        settings.sync.server_url = server;
        let out = w.sync(&snapshot("New"), &settings, false);
        // The settings are taken, and the device still learns that the
        // chain holds its sidebar now.
        let [
            Outcome::Pulled {
                sidebar: None,
                settings: Some(_),
                agreed,
                ..
            },
        ] = out.as_slice()
        else {
            panic!("{:?}", out.iter().map(describe).collect::<Vec<_>>())
        };
        assert_eq!(agreed.as_deref(), Some(&snapshot("New")));
    }

    #[test]
    fn a_sidebar_the_server_holds_is_agreed_on_while_the_settings_fail() {
        let code = SyncCode::generate();
        let server = {
            let keys = code.keys();
            let sidebar = keys.collection_id(SIDEBAR);
            fake_server(move |_, path| {
                if !path.ends_with(sidebar.as_str()) {
                    return (500, "{}".into());
                }
                let plain = serde_json::to_vec(&snapshot("Same")).unwrap();
                let body = CollectionResponse::new(3, keys.seal(&sidebar, 3, &plain).unwrap());
                (200, serde_json::to_string(&body).unwrap())
            })
        };
        let dir = data_dir("held-settings-failed");
        save_code(&dir, &code).unwrap();
        let mut w = Worker::new(dir, SyncSettings::default());
        let mut settings = sync_settings(3);
        settings.sync.server_url = server;
        let out = w.sync(&snapshot("Same"), &settings, false);
        assert!(
            matches!(out.as_slice(), [Outcome::Pushed { versions, agreed: Some(agreed) }, Outcome::Error(_)]
                if versions.is_empty() && **agreed == snapshot("Same")),
            "{:?}",
            out.iter().map(describe).collect::<Vec<_>>()
        );
    }

    /// A worker whose server holds a phone's sent tab in `devices`, at the
    /// version in the first counter (0: nothing there); it answers writes
    /// with the status in the second.
    fn worker_with_sent_tab(name: &str) -> (Worker, Arc<AtomicU64>, Arc<AtomicU64>) {
        let (serving, put) = (Arc::new(AtomicU64::new(3)), Arc::new(AtomicU64::new(200)));
        let code = SyncCode::generate();
        let keys = code.keys();
        let id = keys.collection_id(DEVICES);
        let server = {
            let (serving, put) = (serving.clone(), put.clone());
            fake_server(move |method, _| {
                let version = serving.load(Ordering::SeqCst);
                if method == "PUT" {
                    let status = u16::try_from(put.load(Ordering::SeqCst)).unwrap();
                    return (status, format!(r#"{{"version":{}}}"#, version + 1));
                }
                if version == 0 {
                    return (404, r#"{"error":"no such collection"}"#.into());
                }
                let plain = br#"{"devices":[{"id":"phone","name":"iPhone","tabs":[
                    {"id":"a","url":"https://a.example/","later":true}]}]}"#;
                let sealed = keys.seal(&id, version, plain).unwrap();
                let body = CollectionResponse::new(version, sealed);
                (200, serde_json::to_string(&body).unwrap())
            })
        };
        let dir = data_dir(name);
        save_code(&dir, &code).unwrap();
        let settings = SyncSettings {
            server_url: server,
            ..SyncSettings::default()
        };
        (Worker::new(dir, settings), serving, put)
    }

    fn me() -> Me {
        Me {
            id: "mac".into(),
            name: "Mac".into(),
            taken: Vec::new(),
            applied: Vec::new(),
        }
    }

    #[test]
    fn a_sent_tab_opens_only_once_taking_it_is_written() {
        let (mut w, _, put) = worker_with_sent_tab("devices-claim");
        // Another device keeps writing first: the tab is not this one's.
        put.store(409, Ordering::SeqCst);
        let out = w.devices(&me());
        assert!(
            matches!(out.as_slice(), [Outcome::Error(e)] if e.contains("kept writing first")),
            "{:?}",
            out.iter().map(describe).collect::<Vec<_>>()
        );
        put.store(200, Ordering::SeqCst);
        let out = w.devices(&me());
        let [Outcome::Devices { sent, version, .. }] = out.as_slice() else {
            panic!("{:?}", out.iter().map(describe).collect::<Vec<_>>())
        };
        assert_eq!((sent[0].id.as_str(), *version), ("a", 4));
    }

    #[test]
    fn an_older_devices_collection_is_refused() {
        let (mut w, serving, _) = worker_with_sent_tab("devices-rollback");
        w.settings.versions.insert(DEVICES.into(), 5);
        // Replayed, or deleted: either could bring a finished handover back.
        for old in [3, 0] {
            serving.store(old, Ordering::SeqCst);
            let out = w.devices(&me());
            assert!(
                matches!(out.as_slice(), [Outcome::Error(e)] if e.contains("not going back")),
                "{old}: {:?}",
                out.iter().map(describe).collect::<Vec<_>>()
            );
        }
        // What the worker wrote counts too, before the app has stored it.
        w.settings.versions.clear();
        serving.store(6, Ordering::SeqCst);
        assert!(matches!(
            w.devices(&me()).as_slice(),
            [Outcome::Devices { version: 7, .. }]
        ));
        assert!(
            matches!(w.devices(&me()).as_slice(), [Outcome::Error(e)] if e.contains("not going back"))
        );
    }

    #[test]
    fn older_server_versions_never_replace_newer_local_data() {
        // The server answers every GET with an authentic ciphertext of the
        // version in `serving`; PUTs are counted.
        let serving = Arc::new(AtomicU64::new(3));
        let puts = Arc::new(AtomicUsize::new(0));
        let (worker_code, server) = {
            let code = SyncCode::generate();
            let keys = code.keys();
            let (serving, puts) = (serving.clone(), puts.clone());
            let ids = [
                (keys.collection_id(SIDEBAR), true),
                (keys.collection_id(SETTINGS), false),
            ];
            let server = fake_server(move |method, path| {
                if method == "PUT" {
                    puts.fetch_add(1, Ordering::SeqCst);
                    return (409, r#"{"error":"version conflict"}"#.into());
                }
                let version = serving.load(Ordering::SeqCst);
                let (id, sidebar) = ids
                    .iter()
                    .find(|(id, _)| path.ends_with(id.as_str()))
                    .unwrap();
                let plain = if *sidebar {
                    serde_json::to_vec(&snapshot(&format!("v{version}"))).unwrap()
                } else {
                    serde_json::to_vec(&SyncedSettings::from(&Settings::default())).unwrap()
                };
                let sealed = keys.seal(id, version, &plain).unwrap();
                let body = CollectionResponse::new(version, sealed);
                (200, serde_json::to_string(&body).unwrap())
            });
            (code, server)
        };
        let dir = data_dir("rollback");
        save_code(&dir, &worker_code).unwrap();
        let mut w = Worker::new(dir, SyncSettings::default());
        let at = |versions: u64| {
            let mut s = sync_settings(versions);
            s.sync.server_url.clone_from(&server);
            s
        };
        let pulled_title = |out: &[Outcome]| match out {
            [
                Outcome::Pulled {
                    sidebar: Some(sb), ..
                },
            ] => Some(sb.spaces[0].title.clone()),
            _ => None,
        };

        // This device is at 5; the server offers 3: refused, nothing written.
        let out = w.sync(&snapshot("Local"), &at(5), false);
        assert!(
            out.iter()
                .all(|o| matches!(o, Outcome::Error(e) if e.contains("not going back"))),
            "{:?}",
            out.iter().map(describe).collect::<Vec<_>>()
        );
        assert_eq!(puts.load(Ordering::SeqCst), 0);

        // Version 6 is newer and taken.
        serving.store(6, Ordering::SeqCst);
        assert_eq!(
            pulled_title(&w.sync(&snapshot("Local"), &at(5), false)).as_deref(),
            Some("v6")
        );

        // A job built before that pull was applied (version 5) must not be
        // written over it as version 7: nothing is written.
        let out = w.sync(&snapshot("Stale"), &at(5), false);
        assert!(
            out.iter()
                .all(|o| matches!(o, Outcome::Pushed { versions, .. } if versions.is_empty()))
        );
        assert_eq!(puts.load(Ordering::SeqCst), 0);

        // A job still carrying version 2 (the UI thread has not applied the
        // pull yet) must not take version 4: this device has seen 6.
        serving.store(4, Ordering::SeqCst);
        let out = w.sync(&snapshot("Local"), &at(2), false);
        assert_eq!(pulled_title(&out), None);
        assert!(
            out.iter()
                .any(|o| matches!(o, Outcome::Error(e) if e.contains("has 6")))
        );
    }

    #[test]
    fn what_the_server_already_has_is_not_written_again() {
        // The server holds version 3 of both collections: the sidebar "Same"
        // and the default settings. PUTs are counted and acknowledged.
        let puts = Arc::new(AtomicUsize::new(0));
        let code = SyncCode::generate();
        let server = {
            let keys = code.keys();
            let puts = puts.clone();
            let ids = [
                (keys.collection_id(SIDEBAR), true),
                (keys.collection_id(SETTINGS), false),
            ];
            fake_server(move |method, path| {
                if method == "PUT" {
                    puts.fetch_add(1, Ordering::SeqCst);
                    return (200, r#"{"version":4}"#.into());
                }
                let (id, sidebar) = ids
                    .iter()
                    .find(|(id, _)| path.ends_with(id.as_str()))
                    .unwrap();
                let plain = if *sidebar {
                    serde_json::to_vec(&snapshot("Same")).unwrap()
                } else {
                    serde_json::to_vec(&SyncedSettings::from(&Settings::default())).unwrap()
                };
                let body = CollectionResponse::new(3, keys.seal(id, 3, &plain).unwrap());
                (200, serde_json::to_string(&body).unwrap())
            })
        };
        let dir = data_dir("unchanged");
        save_code(&dir, &code).unwrap();
        let mut w = Worker::new(dir, SyncSettings::default());
        let mut settings = sync_settings(3);
        settings.sync.server_url = server;

        // Round after round with the same data: nothing is written, and
        // the device knows that it and the chain agree on this sidebar.
        for _ in 0..3 {
            let out = w.sync(&snapshot("Same"), &settings, false);
            assert!(
                matches!(out.as_slice(), [Outcome::Pushed { versions, agreed: Some(agreed) }]
                    if versions.is_empty() && **agreed == snapshot("Same")),
                "{:?}",
                out.iter().map(describe).collect::<Vec<_>>()
            );
        }
        assert_eq!(puts.load(Ordering::SeqCst), 0);

        // A sidebar that differs only by what this device keeps of its own
        // since it took the chain's (`sidebar_unchanged`) is not written
        // either: it holds nothing new for the other devices.
        let out = w.sync(&snapshot("Kept here"), &settings, true);
        assert!(
            matches!(out.as_slice(), [Outcome::Pushed { versions, agreed: None }] if versions.is_empty())
        );
        assert_eq!(puts.load(Ordering::SeqCst), 0);

        // Nor is a sidebar without spaces (all of them kept on this device):
        // no device would take it.
        let mut kept_here = snapshot("Kept here");
        kept_here.spaces.clear();
        let out = w.sync(&kept_here, &settings, false);
        assert!(
            matches!(out.as_slice(), [Outcome::Pushed { versions, .. }] if versions.is_empty())
        );
        assert_eq!(puts.load(Ordering::SeqCst), 0);

        // A changed sidebar is written; the unchanged settings are not.
        let out = w.sync(&snapshot("Renamed"), &settings, false);
        let [Outcome::Pushed { versions, agreed }] = out.as_slice() else {
            panic!("{:?}", out.iter().map(describe).collect::<Vec<_>>())
        };
        assert_eq!(versions, &[(SIDEBAR.into(), 4)]);
        assert_eq!(agreed.as_deref(), Some(&snapshot("Renamed")));
        assert_eq!(puts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_device_that_joined_does_not_write_into_an_empty_chain() {
        let puts = Arc::new(AtomicUsize::new(0));
        let counted = puts.clone();
        let server = fake_server(move |method, _| {
            if method == "PUT" {
                counted.fetch_add(1, Ordering::SeqCst);
                return (200, r#"{"version":1}"#.into());
            }
            (404, r#"{"error":"no such collection"}"#.into())
        });
        let (mut w, _) = worker_in_chain("joined", &server);
        // It joined; the device that started the chain has not written yet.
        w.settings.started_here = false;
        let out = w.sync(&snapshot("Mine"), &settings_with(&w.settings), false);
        assert!(
            out.iter()
                .all(|o| matches!(o, Outcome::Error(e) if e.contains("holds no data yet"))),
            "{:?}",
            out.iter().map(describe).collect::<Vec<_>>()
        );
        assert_eq!(puts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_sidebar_without_spaces_is_refused_on_every_round() {
        let code = SyncCode::generate();
        let server = {
            let keys = code.keys();
            let ids = [
                (keys.collection_id(SIDEBAR), true),
                (keys.collection_id(SETTINGS), false),
            ];
            fake_server(move |_, path| {
                let (id, sidebar) = ids
                    .iter()
                    .find(|(id, _)| path.ends_with(id.as_str()))
                    .unwrap();
                let plain = if *sidebar {
                    serde_json::to_vec(&SidebarSnapshot::new()).unwrap()
                } else {
                    serde_json::to_vec(&SyncedSettings::from(&Settings::default())).unwrap()
                };
                let body = CollectionResponse::new(3, keys.seal(id, 3, &plain).unwrap());
                (200, serde_json::to_string(&body).unwrap())
            })
        };
        let dir = data_dir("no-spaces");
        save_code(&dir, &code).unwrap();
        let mut w = Worker::new(dir, SyncSettings::default());
        let mut settings = sync_settings(2);
        settings.sync.server_url = server;
        for _ in 0..2 {
            let out = w.sync(&snapshot("Mine"), &settings, false);
            assert!(
                !out.iter().any(|o| matches!(
                    o,
                    Outcome::Pulled {
                        sidebar: Some(_),
                        ..
                    }
                )),
                "the sidebar is not taken"
            );
            // The reason comes last, after what went through, so it stays.
            assert!(
                matches!(out.last(), Some(Outcome::Error(e)) if e.contains("no spaces")),
                "{:?}",
                out.iter().map(describe).collect::<Vec<_>>()
            );
        }
        assert!(!w.known.contains_key(SIDEBAR), "and not counted as seen");
    }

    #[test]
    fn a_second_chain_is_not_started_over_the_first() {
        let server = fake_server(|_, _| (201, String::new()));
        let (mut w, code) = worker_in_chain("second", &server);
        let out = w.create(&server, None);
        assert!(
            matches!(out.as_slice(), [Outcome::Error(e)] if e.contains("already")),
            "{:?}",
            out.iter().map(describe).collect::<Vec<_>>()
        );
        let kept = load_code(&w.data_dir).expect("the code is still there");
        assert_eq!(kept.to_base64(), code.to_base64());
    }

    #[test]
    fn joining_a_chain_again_calls_off_its_pending_deletion() {
        let server = fake_server(|_, _| (204, String::new()));
        let dir = data_dir("rejoin");
        let code = SyncCode::generate();
        save_pending_delete(&dir, &server, &code).unwrap();
        let mut w = Worker::new(dir.clone(), SyncSettings::default());
        let out = w.join(&server, &code.words());
        assert!(
            matches!(out.as_slice(), [Outcome::Connected { created: false, .. }]),
            "{:?}",
            out.iter().map(describe).collect::<Vec<_>>()
        );
        assert!(load_pending_delete(&dir).is_none());
    }

    #[test]
    fn a_sidebar_in_a_newer_format_is_neither_taken_nor_written_over() {
        let code = SyncCode::generate();
        let keys = code.keys();
        let puts = Arc::new(AtomicUsize::new(0));
        let counted = puts.clone();
        let ids = [keys.collection_id(SIDEBAR), keys.collection_id(SETTINGS)];
        let server = fake_server(move |method, path| {
            if method == "PUT" {
                counted.fetch_add(1, Ordering::SeqCst);
                return (200, r#"{"version":4}"#.into());
            }
            let Some(id) = ids.iter().find(|id| path.ends_with(id.as_str())) else {
                return (404, "{}".into());
            };
            let mut newer = snapshot("Newer");
            newer.version = SidebarSnapshot::VERSION + 1;
            let plain = if *id == ids[0] {
                serde_json::to_vec(&newer).unwrap()
            } else {
                serde_json::to_vec(&SyncedSettings::from(&Settings::default())).unwrap()
            };
            let body = CollectionResponse::new(3, keys.seal(id, 3, &plain).unwrap());
            (200, serde_json::to_string(&body).unwrap())
        });
        let dir = data_dir("newer-format");
        save_code(&dir, &code).unwrap();
        let mut w = Worker::new(dir, SyncSettings::default());
        let mut settings = sync_settings(2);
        settings.sync.server_url.clone_from(&server);
        // Round after round: the user is told, and nothing of the sidebar
        // is taken or written.
        for _ in 0..2 {
            let out = w.sync(&snapshot("Local"), &settings, false);
            assert!(
                out.iter()
                    .any(|o| matches!(o, Outcome::Error(e) if e.contains("newer version"))),
                "{:?}",
                out.iter().map(describe).collect::<Vec<_>>()
            );
            assert!(!out.iter().any(|o| matches!(
                o,
                Outcome::Pulled {
                    sidebar: Some(_),
                    ..
                }
            )));
        }
        assert_eq!(puts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn data_this_version_cannot_read_is_not_taken() {
        let code = SyncCode::generate();
        let keys = code.keys();
        let ids = [keys.collection_id(SIDEBAR), keys.collection_id(SETTINGS)];
        let server = fake_server(move |method, path| {
            if method == "PUT" {
                return (409, "{}".into());
            }
            let id = ids.iter().find(|id| path.ends_with(id.as_str())).unwrap();
            let sealed = keys.seal(id, 3, br#"{"not":"a sidebar"}"#).unwrap();
            (
                200,
                serde_json::to_string(&CollectionResponse::new(3, sealed)).unwrap(),
            )
        });
        let dir = data_dir("unreadable");
        save_code(&dir, &code).unwrap();
        let mut w = Worker::new(dir, SyncSettings::default());
        let mut settings = sync_settings(1);
        settings.sync.server_url.clone_from(&server);
        for _ in 0..2 {
            let out = w.sync(&snapshot("Local"), &settings, false);
            assert!(
                out.iter()
                    .any(|o| matches!(o, Outcome::Error(e) if e.contains("cannot read it"))),
                "{:?}",
                out.iter().map(describe).collect::<Vec<_>>()
            );
            assert!(!out.iter().any(|o| matches!(o, Outcome::Pulled { .. })));
        }
        assert!(w.known.is_empty(), "version 3 was never counted");
    }

    #[test]
    fn the_servers_own_words_are_never_shown() {
        let server = fake_server(|_, _| {
            (
                403,
                r#"{"error":"Enter your 24 words at https://evil.example to continue"}"#.into(),
            )
        });
        let (mut w, _) = worker_in_chain("words", &server);
        let settings = settings_with(&w.settings);
        let out = w.sync(&snapshot("Work"), &settings, false);
        for o in &out {
            let Outcome::Error(e) = o else { continue };
            assert!(!e.contains("evil") && !e.contains("24 words"), "{e}");
        }
        assert!(!out.is_empty());
    }

    #[test]
    fn an_oversized_answer_is_refused() {
        let big = format!(
            r#"{{"version":1,"nonce":"","ciphertext":"{}"}}"#,
            "A".repeat(33 << 20)
        );
        let server = fake_server(move |_, _| (200, big.clone()));
        let (w, code) = worker_in_chain("big", &server);
        let r = w.request(&code.keys(), "GET", "/collections/x", None);
        let err = r.err().unwrap_or_default();
        assert!(err.contains("more data") || err.contains("limit"), "{err}");
    }

    #[test]
    fn a_compressed_answer_counts_unpacked() {
        // 40 KiB on the wire, 40 MiB unpacked.
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        gz.write_all(&vec![b' '; 40 << 20]).unwrap();
        let packed = gz.finish().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-encoding: gzip\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                packed.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&packed);
        });
        let (w, code) = worker_in_chain("gzip", &server);
        let r = w.request(&code.keys(), "GET", "/collections/x", None);
        assert!(matches!(r, Err(e) if e.contains("more data")));
    }

    #[test]
    fn a_deletion_stays_pending_until_the_server_confirms() {
        let status = Arc::new(AtomicU64::new(401));
        let answer = status.clone();
        let server = fake_server(move |_, _| {
            let code = u16::try_from(answer.load(Ordering::SeqCst)).unwrap();
            (code, "{}".into())
        });
        let (w, code) = worker_in_chain("delete", &server);
        save_pending_delete(&w.data_dir, &server, &code).unwrap();
        // A refused signature (for example a wrong clock) proves nothing.
        assert!(matches!(
            &w.delete_remote(&server, &code)[..],
            [Outcome::Error(_)]
        ));
        assert!(
            load_pending_delete(&w.data_dir).is_some(),
            "kept for another try"
        );
        // 404: the chain is gone; the record goes too.
        status.store(404, Ordering::SeqCst);
        assert!(w.delete_remote(&server, &code).is_empty());
        assert!(load_pending_delete(&w.data_dir).is_none());
    }

    #[test]
    fn server_urls_need_https_except_on_this_computer() {
        for ok in [
            "https://sync.example.org",
            "https://sync.example.org/magnetite/",
            "http://localhost:8787",
            "http://127.0.0.1:8787",
            "http://[::1]:8787",
        ] {
            assert_eq!(
                server_base(ok).as_deref(),
                Ok(ok.trim_end_matches('/')),
                "{ok}"
            );
        }
        for bad in [
            "http://sync.example.org",
            "http://localhost.example.org",
            "http://127.0.0.1.nip.io",
            "http://127.0.0.1@sync.example.org",
            "https://user:secret@sync.example.org",
            "https://sync.example.org/?redirect=x",
            "ftp://sync.example.org",
            "sync.example.org",
            "",
        ] {
            assert!(server_base(bad).is_err(), "{bad}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_sync_key_is_only_ever_readable_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = data_dir("key-mode");
        // Even over an older, world-readable file.
        std::fs::write(secrets_path(&dir), "{}").unwrap();
        std::fs::set_permissions(secrets_path(&dir), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        let code = SyncCode::generate();
        save_code(&dir, &code).unwrap();
        let mode = std::fs::metadata(secrets_path(&dir))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(load_code(&dir), Some(code));
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "no temporary file left"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn leaving_does_not_wait_for_a_stalled_server() {
        // Takes the connection but never answers.
        let stalled = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", stalled.local_addr().unwrap());
        let (w, _) = worker_in_chain("stalled", &server);
        let dir = w.data_dir.clone();
        let (tx, rx) = channel();
        let client = SyncClient::spawn(
            dir.clone(),
            w.settings.clone(),
            Arc::new(move |o| {
                tx.send(describe(&o)).ok();
            }),
        );
        let sync = || Job::Sync {
            snapshot: Box::new(snapshot("Work")),
            settings: Box::new(settings_with(&w.settings)),
            sidebar_unchanged: false,
            me: Me::default(),
            archive: Vec::new(),
            archive_unchanged: true,
        };
        client.submit(sync());
        let _connection = stalled.accept().unwrap(); // the worker is now stuck

        let started = std::time::Instant::now();
        for _ in 1..20 {
            client.submit(sync());
        }
        client.submit(Job::Leave);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(rx.try_recv().as_deref(), Ok("Disconnected"));
        assert!(!signed_in(&dir));
        assert!(
            lock(&client.shared.pending).is_none(),
            "queued sync dropped"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The server keeps its data in Postgres: this test needs one in
    /// `DATABASE_URL`, as magnetite-sync's own tests do (CI starts one on
    /// Linux and runs it with `--ignored`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "needs a Postgres in DATABASE_URL"]
    async fn two_devices_share_a_chain_through_a_real_server() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL names a Postgres");
        let state = magnetite_sync::AppState {
            db: magnetite_sync::Db::connect(&url).await.unwrap(),
            registration: magnetite_sync::Registration::Open,
            retention_days: None,
            // The database may hold chains of earlier runs.
            limits: magnetite_sync::Limits {
                max_chains: u32::MAX,
                max_new_chains_per_day: 0,
                ..magnetite_sync::Limits::default()
            },
        };
        let (addr, _handle) = magnetite_sync::serve_ephemeral(state).await.unwrap();
        let server = format!("http://{addr}");
        let (d1, d2, d3) = (data_dir("a"), data_dir("b"), data_dir("c"));

        let result = tokio::task::spawn_blocking(move || {
            // Device A starts a chain and pushes.
            let mut a = Worker::new(d1.clone(), SyncSettings::default());
            let out = a.create(&server, None);
            assert!(
                matches!(out[0], Outcome::Connected { .. }),
                "create: {}",
                describe(&out[0])
            );
            let words = code_words(&d1).expect("code stored");
            let out = a.sync(&snapshot("Work"), &settings_with(&a.settings), false);
            let Outcome::Pushed { versions, .. } = &out[0] else {
                panic!("expected push, got {}", describe(&out[0]))
            };
            assert_eq!(versions.len(), 2);
            a.settings.versions = versions.iter().cloned().collect();

            // Device B joins with the 24 words and pulls A's sidebar.
            let mut b = Worker::new(d2.clone(), SyncSettings::default());
            let out = b.join(&server, &words.to_uppercase());
            assert!(
                matches!(out[0], Outcome::Connected { .. }),
                "join: {}",
                describe(&out[0])
            );
            let out = b.sync(&snapshot("Local only"), &settings_with(&b.settings), false);
            let Outcome::Pulled {
                sidebar: Some(sb),
                versions,
                ..
            } = &out[0]
            else {
                panic!("expected pull, got {}", describe(&out[0]))
            };
            assert_eq!(sb.spaces[0].title, "Work");
            b.settings.versions = versions.iter().cloned().collect();

            // B changes and pushes; A pulls the change.
            let out = b.sync(
                &snapshot("Work renamed"),
                &settings_with(&b.settings),
                false,
            );
            assert!(
                matches!(out[0], Outcome::Pushed { .. }),
                "push from b: {}",
                describe(&out[0])
            );
            let out = a.sync(&snapshot("Work"), &settings_with(&a.settings), false);
            let Outcome::Pulled {
                sidebar: Some(sb), ..
            } = &out[0]
            else {
                panic!("expected pull on a, got {}", describe(&out[0]))
            };
            assert_eq!(sb.spaces[0].title, "Work renamed");

            // A code that is not on the server cannot join.
            let mut c = Worker::new(d3.clone(), SyncSettings::default());
            let out = c.join(&server, &SyncCode::generate().words());
            assert!(
                matches!(&out[0], Outcome::Error(e) if e.contains("No sync chain")),
                "{}",
                describe(&out[0])
            );

            // Deleting the chain removes it for everyone.
            assert!(
                a.delete_remote(&a.settings.server_url, &load_code(&d1).unwrap())
                    .is_empty()
            );
            let mut again = Worker::new(d2.clone(), SyncSettings::default());
            assert!(matches!(&again.join(&server, &words)[0], Outcome::Error(_)));
            for d in [d1, d2, d3] {
                let _ = std::fs::remove_dir_all(d);
            }
        })
        .await;
        result.unwrap();
    }
}

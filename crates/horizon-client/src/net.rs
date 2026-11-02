
//! Multiplayer session: a network thread that connects to a server, logs in,
//! runs the configuration phase and then forwards decoded play packets to the
//! game thread. Keep-alives are answered on the network thread; pings wait for
//! the game thread, like ClientCommonPacketListenerImpl.handlePing, so a pong
//! cannot overtake an earlier world update or teleport acknowledgement.
//! Online-mode servers need the Microsoft account the launcher signed in
//! (crate::auth): the login joins the server through the session service and
//! encrypts the connection (ClientHandshakePacketListenerImpl.handleHello).
use crate::auth::AuthSession;
use horizon_protocol::{
    PROTOCOL_VERSION,
    connection::{ConnReader, ConnWriter, Connection, Counters},
    crypto::{generate_secret, offline_uuid, server_hash},
    packets::{Empty, configuration as cfg, handshake, login, play},
    registry::Dynamic,
    rsa::RsaPublicKey,
    servers::parse_address,
    text::{Language, plain, translatable},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    thread::JoinHandle,
    time::Duration,
};

/// `CommonListenerCookie.serverCookies`: cookies the server asked the client
/// to store (`ClientboundStoreCookiePacket`), answered to
/// `ClientboundCookieRequestPacket`. A `transfer` carries them to the next
/// connection (`TransferState`), so a proxy's backend switch keeps them.
pub type Cookies = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// What the network thread reports to the game.
pub enum Event {
    /// Progress text for the connecting screen.
    Status(String),
    /// The server's `minecraft:brand` custom payload.
    Brand(String),
    /// The server runs mods' quiet commands (`horizon_protocol::quiet`).
    QuietCommands,
    /// Configuration finished: the play state begins with these registries.
    Joined {
        registries: Arc<Dynamic>,
        uuid: u128,
        name: String,
    },
    Packet(Box<play::Clientbound>),
    /// A packet that failed to decode (skipped, like vanilla's skippable packets).
    DecodeError(String),
    /// The server sent us back to configuration and finished it again.
    Reconfigured(Arc<Dynamic>),
    /// ClientboundResourcePackPushPacket, in configuration or in play.
    PackPush(cfg::ResourcePackPush),
    /// ClientboundResourcePackPopPacket: one pack, or every pack.
    PackPop(Option<u128>),
    /// `show_dialog` in configuration: the dialog itself.
    ShowDialog(horizon_world::nbt::Tag),
    /// `clear_dialog` in configuration.
    ClearDialog,
    /// ClientboundCodeOfConductPacket: the game shows it and answers with
    /// `Session::accept_code_of_conduct` or a disconnect; until then the
    /// server waits in configuration.
    CodeOfConduct(String),
    /// `ClientboundTransferPacket`: the server sent us to another address.
    /// The connection ends right after; the game reconnects there with the
    /// stored cookies.
    Transfer {
        host: String,
        port: i32,
    },
    /// `ClientboundServerLinksPacket`: the links the pause menu's "Server
    /// Links..." lists (ClientCommonPacketListenerImpl.handleServerLinks).
    ServerLinks(Vec<cfg::ServerLink>),
    /// ModSync: content block definitions the server pushed on the
    /// `horizon:modsync` channel during configuration (how many).
    /// Phase 1 reports them; the game thread decides what to do.
    ModSync(usize),
    /// The connection ended. `from_server`: the server gave the reason (or
    /// the connection was refused for good), rather than the connection
    /// failing (auto-reconnect retries only failures and shutdowns).
    Disconnected {
        reason: String,
        from_server: bool,
    },
}

pub struct Session {
    events: Receiver<Event>,
    writer: Arc<Mutex<Option<ConnWriter>>>,
    /// The connection is in configuration (else in play). Changed only
    /// with `writer` locked, as the packet that switches is sent.
    configuring: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub address: String,
    pub name: String,
    /// ViaHorizonPlus step 1: the version the status probe read off the
    /// server before login, `(protocol, version_name)`. `None` when the
    /// server speaks the client's own 26.3, the address is a friend's
    /// world or local, or the probe failed (a probe failure never blocks
    /// the join). Shared with the net thread, which runs the probe before
    /// opening the login connection (a synchronous probe here would freeze
    /// the game thread for the probe timeout).
    /// RU: результат пробы версии сервера до входа; общий с сетевым потоком,
    /// который и делает пробу до login-соединения.
    pub probed: Arc<Mutex<Option<(i32, String)>>>,
    pub counters: Arc<Mutex<Option<Arc<Counters>>>>,
    /// The cookies this server stored on us (`ClientboundStoreCookiePacket`).
    pub cookies: Cookies,
    /// HORIZON_NET_DEBUG=1 logs every packet sent from the game thread.
    debug: bool,
}
impl Session {
    /// Start connecting in the background, as `auth`'s account when there is one.
    pub fn connect(
        address: &str,
        name: &str,
        auth: Option<Arc<AuthSession>>,
        view_distance: i8,
        lang: Arc<Language>,
        resources: PathBuf,
    ) -> Self {
        Self::start(
            address,
            name,
            auth,
            view_distance,
            lang,
            None,
            None,
            resources,
        )
    }
    /// `ConnectScreen.startConnecting` for a `transfer`: the cookies the
    /// server stored carry to the new connection (`TransferState`).
    pub fn connect_transferred(
        address: &str,
        name: &str,
        auth: Option<Arc<AuthSession>>,
        view_distance: i8,
        lang: Arc<Language>,
        cookies: Cookies,
        resources: PathBuf,
    ) -> Self {
        Self::start(
            address,
            name,
            auth,
            view_distance,
            lang,
            None,
            Some(cookies),
            resources,
        )
    }
    /// Join an integrated server through memory (`Connection.connectToLocalServer`).
    pub fn connect_local(
        pipe: horizon_protocol::connection::Pipe,
        name: &str,
        view_distance: i8,
        lang: Arc<Language>,
        resources: PathBuf,
    ) -> Self {
        Self::start(
            "local",
            name,
            None,
            view_distance,
            lang,
            Some(pipe),
            None,
            resources,
        )
    }
    fn start(
        address: &str,
        name: &str,
        auth: Option<Arc<AuthSession>>,
        view_distance: i8,
        lang: Arc<Language>,
        local: Option<horizon_protocol::connection::Pipe>,
        cookies: Option<Cookies>,
        resources: PathBuf,
    ) -> Self {
        let (send, events) = channel();
        let writer = Arc::new(Mutex::new(None));
        let configuring = Arc::new(AtomicBool::new(true));
        let counters = Arc::new(Mutex::new(None));
        // Connection.initiateServerboundPlayConnection: a transfer's
        // handshake says so (ClientIntent.TRANSFER).
        let transferred = cookies.is_some();
        let cookies = cookies.unwrap_or_default();
        let (thread_writer, thread_counters, thread_cookies) =
            (writer.clone(), counters.clone(), cookies.clone());
        let thread_configuring = configuring.clone();
        let (address_owned, name_owned) = (address.to_owned(), name.to_owned());
        let probed: Arc<Mutex<Option<(i32, String)>>> = Arc::new(Mutex::new(None));
        let thread_probed = probed.clone();
        // ViaHorizonPlus step 1: probe the server's version before the
        // login connection is opened. Only real server addresses; a
        // friend's world and the integrated server speak our own protocol.
        // Runs on the net thread so the game thread never waits.
        // RU: проба версии сервера до login; на сетевом потоке, игровой
        // поток не ждёт.
        let thread = std::thread::Builder::new()
            .name("horizon-net".into())
            .spawn(move || {
                if local.is_none() && !address_owned.starts_with(FRIEND) {
                    if let Some((protocol, name)) = probe_server_version(&address_owned) {
                        *thread_probed.lock().unwrap() = Some((protocol, name.clone()));
                        status(
                            &send,
                            &format!(
                                "ViaHorizonPlus: server runs {name} (protocol {protocol}), \
                                 Horizon speaks 26.3 (777). Translation is being prepared."
                            ),
                        );
                    }
                }
                let result = run(
                    local,
                    &address_owned,
                    &name_owned,
                    auth.as_deref(),
                    view_distance,
                    &lang,
                    &send,
                    &thread_writer,
                    &thread_configuring,
                    &thread_counters,
                    &thread_cookies,
                    transferred,
                    // Speak the server's protocol in the handshake, as
                    // ViaHorizonPlus does: fake our version to match the probe.
                    // RU: подмена версии в handshake на пробленную протокол-версию.
                    thread_probed.lock().unwrap().as_ref().map(|&(p, _)| p),
                    &resources,
                );
                let from_server = result.is_ok();
                let reason = result.unwrap_or_else(|e| e);
                if let Some(w) = thread_writer.lock().unwrap().take() {
                    w.shutdown();
                }
                let _ = send.send(Event::Disconnected {
                    reason,
                    from_server,
                });
            })
            .ok();
        let debug = std::env::var_os("HORIZON_NET_DEBUG").is_some();
        Self {
            events,
            writer,
            configuring,
            thread,
            address: address.to_owned(),
            name: name.to_owned(),
            probed,
            counters,
            cookies,
            debug,
        }
    }
    /// Everything received since the last poll.
    pub fn poll(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }
    /// Send a play packet; silently dropped once the connection is gone,
    /// and while the server has the connection back in configuration (a
    /// proxy switching servers). Vanilla's `handleConfigurationStart` clears
    /// the level before it acknowledges, so no play packet follows the
    /// acknowledgement; one that did would reach the server as a
    /// configuration packet, and Velocity drops such a client.
    pub fn send(&self, packet: play::Serverbound) {
        if self.debug
            && !matches!(
                packet,
                play::Serverbound::ClientTickEnd(_)
                    | play::Serverbound::MovePlayerPos(_)
                    | play::Serverbound::MovePlayerPosRot(_)
            )
        {
            eprintln!("[net] -> {packet:?}");
        }
        let mut writer = self.writer.lock().unwrap();
        if self.configuring.load(Ordering::Relaxed) {
            return;
        }
        if let Some(w) = writer.as_mut()
            && w.send(&packet.encode()).is_err()
        {
            w.shutdown();
        }
    }
    /// ServerboundResourcePackPacket, in whichever phase the connection is.
    pub fn send_pack_status(&self, id: u128, action: i32) {
        let status = cfg::ResourcePackResponse { id, action };
        let mut writer = self.writer.lock().unwrap();
        let body = if self.configuring.load(Ordering::Relaxed) {
            cfg::Serverbound::ResourcePack(status).encode()
        } else {
            play::Serverbound::ResourcePack(status).encode()
        };
        if let Some(w) = writer.as_mut()
            && w.send(&body).is_err()
        {
            w.shutdown();
        }
    }
    /// ServerboundCustomClickActionPacket (a dialog's answer), in whichever
    /// phase the connection is.
    pub fn send_custom_click(&self, id: &str, payload: Option<horizon_world::nbt::Tag>) {
        let click = cfg::CustomClickAction {
            id: id.to_owned(),
            payload,
        };
        let mut writer = self.writer.lock().unwrap();
        let body = if self.configuring.load(Ordering::Relaxed) {
            cfg::Serverbound::CustomClickAction(click).encode()
        } else {
            play::Serverbound::CustomClickAction(click).encode()
        };
        if let Some(w) = writer.as_mut()
            && w.send(&body).is_err()
        {
            w.shutdown();
        }
    }
    /// ServerboundAcceptCodeOfConductPacket, sent in configuration.
    pub fn accept_code_of_conduct(&self) {
        let mut writer = self.writer.lock().unwrap();
        if let Some(w) = writer.as_mut()
            && w.send(&cfg::Serverbound::AcceptCodeOfConduct(Empty).encode())
                .is_err()
        {
            w.shutdown();
        }
    }
    /// `Connection.disconnect`: the socket closes and the game goes on at once.
    /// The reader thread is left to end on its own: on Windows a shutdown
    /// doesn't wake a read already waiting, so joining it held the window until
    /// the server dropped the connection, many seconds later.
    pub fn disconnect(&mut self) {
        if let Some(w) = self.writer.lock().unwrap().take() {
            w.shutdown();
        }
        drop(self.thread.take());
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.disconnect();
    }
}

/// An address of the form `friend:K7QM-3XPD` joins a friend's world through
/// its invite code (horizon-friends): the session goes through a tunnel the
/// client reaches on a loopback port, so everything after is a plain join.
pub const FRIEND: &str = horizon_friends::ADDRESS_PREFIX;

/// An address as players and logs see it: a friend's invite code stays
/// out of it (its secret is in there).
pub fn shown_address(address: &str) -> String {
    match address.strip_prefix(FRIEND) {
        Some(code) => match horizon_friends::invite::parse(code) {
            Some(horizon_friends::invite::Parsed::Short(c)) => {
                format!("Friend's world ({}-****)", c.nameplate)
            }
            _ => "Friend's world".to_owned(),
        },
        None => address.to_owned(),
    }
}

/// The relay the player set (`friendsRelay` in horizon.txt, beside
/// options.txt), else `HORIZON_RELAY_URL`. None by default.
pub fn friends_relay_setting() -> Option<String> {
    let dir = std::env::current_dir().ok()?;
    crate::ui_theme::preference(&dir, "friendsRelay")
        .filter(|r| !r.is_empty())
        .or_else(horizon_friends::relay_url)
}
/// Remember the relay typed on Join a friend (empty forgets it).
pub fn save_friends_relay(relay: &str) {
    if let Ok(dir) = std::env::current_dir() {
        crate::ui_theme::set_preference(&dir, "friendsRelay", (!relay.is_empty()).then_some(relay));
    }
}

/// Reach the friend's world from the code (directly, or through a relay)
/// and open the tunnel: the loopback address to connect to.
fn join_friend(code: &str, send: &Sender<Event>) -> Result<(String, u16), String> {
    use horizon_friends::{
        PathMode,
        guest::{self, GuestConfig},
    };
    status(send, "Finding your friend's world...");
    let tunnel = guest::join(&GuestConfig {
        code: code.to_owned(),
        relay: friends_relay_setting(),
        protocol: PROTOCOL_VERSION,
        game: horizon_protocol::GAME_VERSION.to_owned(),
        path: PathMode::from_env(),
    })
    .map_err(|e| e.to_string())?;
    status(
        send,
        if tunnel.direct {
            "Connected straight to your friend..."
        } else {
            "Connected to your friend through the relay..."
        },
    );
    let (port, _) = tunnel.serve_loopback().map_err(|e| e.to_string())?;
    Ok(("127.0.0.1".to_owned(), port))
}

fn status(send: &Sender<Event>, text: &str) {
    let _ = send.send(Event::Status(text.to_owned()));
}

/// Shown when an online-mode server asks a client without an account to authenticate.
pub const NOT_SIGNED_IN: &str = "This server is in online mode. Sign in with your Microsoft account in the Horizon launcher to join it.";

/// A translation, or the key itself (Language.getOrDefault).
fn tr(lang: &Language, key: &str) -> String {
    lang.get(key).cloned().unwrap_or_else(|| key.to_owned())
}

/// The whole connection. Ok: why it ended for good (the server's reason, or
/// a refusal no retry fixes); Err: the connection failed.
fn run(
    local: Option<horizon_protocol::connection::Pipe>,
    address: &str,
    name: &str,
    auth: Option<&AuthSession>,
    view_distance: i8,
    lang: &Language,
    send: &Sender<Event>,
    shared_writer: &Mutex<Option<ConnWriter>>,
    configuring: &AtomicBool,
    shared_counters: &Mutex<Option<Arc<Counters>>>,
    cookies: &Cookies,
    transferred: bool,
    probed_protocol: Option<i32>,
    resources: &Path,
) -> Result<String, String> {
    let (host, port, mut c) = if let Some(pipe) = local {
        (address.to_owned(), 0, Connection::local(pipe))
    } else {
        let (host, port) = match address.strip_prefix(FRIEND) {
            Some(code) => join_friend(code, send)?,
            None => {
                let Some(target) = parse_address(address) else {
                    return Ok(format!("Invalid server address: {address}"));
                };
                status(send, "Connecting to the server...");
                target
            }
        };
        let c = if address.starts_with(FRIEND) {
            // The friends relay supplies a concrete endpoint, not a Minecraft hostname.
            Connection::connect(&host, port, Duration::from_secs(10))
        } else {
            Connection::connect_server(&host, port, Duration::from_secs(10))
        }
        .map_err(|e| e.to_string())?;
        (host, port, c)
    };
    *shared_counters.lock().unwrap() = Some(c.reader.counters.clone());
    let e = |e: horizon_protocol::Error| e.to_string();
    c.send(
        &handshake::Serverbound::Intention(handshake::Intention {
            // Speak the server's protocol in the handshake so it accepts us
            // before translation: fake our version to the probed one.
            protocol: probed_protocol.unwrap_or(PROTOCOL_VERSION),
            host: host.clone(),
            port,
            intent: if transferred {
                handshake::TRANSFER
            } else {
                handshake::LOGIN
            },
        })
        .encode(),
    )
    .map_err(e)?;
    // Vanilla's login sends the account's profile id, or the offline one.
    let uuid = auth.map_or_else(|| offline_uuid(name), |a| a.uuid);
    c.send(
        &login::Serverbound::Hello(login::ServerHello {
            name: name.to_owned(),
            uuid,
        })
        .encode(),
    )
    .map_err(e)?;
    let profile = loop {
        let (id, payload) = c.recv().map_err(e)?;
        // 767 (1.21.x) login success is [uuid][name][properties], while
        // 26.x writes [name][uuid][properties][session uuid]: decode the
        // old order by hand when we speak the probed 767 protocol.
        // / RU: у 767 успех логина - другой порядок полей; при пробе 767
        // разбираем вручную.
        if probed_protocol == Some(767) && id == 0x02 {
            // [uuid 16][name varint-len str][properties varint count]...
            // / RU: старый порядок полей 1.21.x.
            let mut r = horizon_protocol::buf::Reader::new(&payload);
            let uuid = r.uuid().map_err(|err| format!("profile uuid: {err}"))?;
            let name = r.string(16).map_err(|err| format!("profile name: {err}"))?;
            let game = login::GameProfile {
                name,
                uuid,
                properties: Vec::new(),
            };
            c.send(&login::Serverbound::LoginAcknowledged(Empty).encode())
                .map_err(e)?;
            break game;
        }
        match login::Clientbound::decode(id, &payload).map_err(e)? {
            login::Clientbound::Compression(p) => c.set_compression(p.threshold),
            login::Clientbound::LoginFinished(p) => {
                c.send(&login::Serverbound::LoginAcknowledged(Empty).encode())
                    .map_err(e)?;
                break p.profile;
            }
            login::Clientbound::Disconnect(p) => {
                let reason = serde_json::from_str::<serde_json::Value>(&p.reason_json)
                    .map(|v| json_text(&v, lang))
                    .unwrap_or(p.reason_json);
                return Ok(reason);
            }
            login::Clientbound::Hello(hello) => {
                status(send, &tr(lang, "connect.authorizing"));
                let key = RsaPublicKey::from_der(&hello.public_key)
                    .map_err(|err| format!("Protocol error: {err}"))?;
                let secret = generate_secret().map_err(e)?;
                if hello.should_authenticate {
                    match auth {
                        Some(account) => {
                            let hash = server_hash(&hello.server_id, &secret, &hello.public_key);
                            if let Err(err) = crate::auth::join_server(account, &hash) {
                                let reason = match err.reason() {
                                    Ok(key) => tr(lang, key),
                                    Err(message) => message.to_owned(),
                                };
                                return Err(plain(
                                    &translatable("disconnect.loginFailedInfo", &[&reason]),
                                    lang,
                                ));
                            }
                        }
                        // EN: no account: send the key without Mojang-auth; a
                        // server that is actually offline accepts this, one
                        // that checks rejects it with a clear reason.
                        // RU: без аккаунта шлём ключ без аутентификации:
                        // offline-сервер примет, online отклонит с причиной.
                        None => log::warn!(
                            "Horizon: server asked for authentication with no                              signed-in account; continuing without it"
                        ),
                    }
                }
                status(send, &tr(lang, "connect.encrypting"));
                let key_enc = key.encrypt(&secret).map_err(e)?;
                let challenge_enc = key.encrypt(&hello.challenge).map_err(e)?;
                log::info!(
                    "Horizon: sending Key: secret {}B, challenge {}B (server key {}B)",
                    key_enc.len(),
                    challenge_enc.len(),
                    key.size()
                );
                c.send(
                    &login::Serverbound::Key(login::Key {
                        key: key_enc,
                        challenge: challenge_enc,
                    })
                    .encode(),
                )
                .map_err(e)?;
                c.enable_encryption(&secret);
            }
            login::Clientbound::CustomQuery(q) => {
                c.send(
                    &login::Serverbound::CustomQueryAnswer(login::CustomQueryAnswer {
                        transaction: q.transaction,
                        payload: None,
                    })
                    .encode(),
                )
                .map_err(e)?;
            }
            // ClientHandshakePacketListenerImpl.handleRequestCookie: the
            // cookies a transfer carried over (none on a plain join).
            login::Clientbound::CookieRequest(q) => {
                let payload = cookies.lock().unwrap().get(&q.key).cloned();
                c.send(
                    &login::Serverbound::CookieResponse(login::CookieResponse {
                        key: q.key,
                        payload,
                    })
                    .encode(),
                )
                .map_err(e)?;
            }
            login::Clientbound::Unknown(..) => {}
        }
    };
    status(send, "Joining world...");
    let mut info_packet =
        cfg::Serverbound::ClientInformation(cfg::ClientInformation::new(view_distance)).encode();
    if probed_protocol == Some(767) {
        // 767 has no particle status field: the default 0 encodes as one
        // 0x00 byte at the end - cut it off.
        // / RU: у 767 нет particle status: отрезаем хвостовой байт.
        if info_packet.last() == Some(&0) {
            info_packet.pop();
        }
    }
    c.send(&info_packet).map_err(e)?;
    c.send(
        &cfg::Serverbound::CustomPayload(cfg::CustomPayload {
            channel: "minecraft:brand".into(),
            data: brand(),
        })
        .encode(),
    )
    .map_err(e)?;
    let (mut reader, writer) = c.split();
    *shared_writer.lock().unwrap() = Some(writer);
    let registries = configure(
        &mut reader,
        shared_writer,
        configuring,
        lang,
        send,
        cookies,
        resources,
        probed_protocol,
    )?;
    let _ = send.send(Event::Joined {
        registries: Arc::new(registries),
        uuid: profile.uuid,
        name: profile.name,
    });
    // Vanilla registers the channels it listens on with `minecraft:register`
    // (NUL-separated channel names in the payload), so plugin-messenger
    // servers (Paper) deliver their server->client plugin messages. We
    // listen for ModSync; declaring it keeps the Messenger path alive.
    // RU: ванильный minecraft:register — без него Paper не шлёт plugin messages.
    {
        let channels = format!("{}\0", crate::modsync::CHANNEL);
        if let Some(w) = shared_writer.lock().unwrap().as_mut() {
            let _ = w.send(
                &play::Serverbound::CustomPayload(cfg::CustomPayload {
                    channel: "minecraft:register".into(),
                    data: channels.into_bytes(),
                })
                .encode(),
            );
        }
    }
    receive_play(
        &mut reader,
        shared_writer,
        configuring,
        lang,
        send,
        cookies,
        resources,
        probed_protocol,
    )
}

fn receive_play(
    reader: &mut ConnReader,
    shared_writer: &Mutex<Option<ConnWriter>>,
    configuring: &AtomicBool,
    lang: &Language,
    send: &Sender<Event>,
    cookies: &Cookies,
    resources: &Path,
    probed_protocol: Option<i32>,
) -> Result<String, String> {
    loop {
        let (id, payload) = reader.recv().map_err(|e| e.to_string())?;
        let packet = match play::Clientbound::decode(id, &payload) {
            Ok(p) => p,
            Err(err) => {
                let name = horizon_protocol::ids::name(
                    horizon_protocol::State::Play,
                    horizon_protocol::Direction::Clientbound,
                    id,
                )
                .unwrap_or("?");
                let _ = send.send(Event::DecodeError(format!("{name}: {err}")));
                continue;
            }
        };
        let reply = |p: play::Serverbound| {
            if let Some(w) = shared_writer.lock().unwrap().as_mut() {
                let _ = w.send(&p.encode());
            }
        };
        match packet {
            play::Clientbound::KeepAlive(k) => reply(play::Serverbound::KeepAlive(k)),
            play::Clientbound::BundleDelimiter(_) => {}
            play::Clientbound::ResourcePackPush(p) => {
                let _ = send.send(Event::PackPush(p));
            }
            play::Clientbound::ResourcePackPop(p) => {
                let _ = send.send(Event::PackPop(p.id));
            }
            play::Clientbound::StartConfiguration(_) => {
                if let Some(w) = shared_writer.lock().unwrap().as_mut() {
                    let _ = w.send(&play::Serverbound::ConfigurationAcknowledged(Empty).encode());
                    configuring.store(true, Ordering::Relaxed);
                }
                let registries = configure(
                    reader,
                    shared_writer,
                    configuring,
                    lang,
                    send,
                    cookies,
                    resources,
                    probed_protocol,
                )?;
                let _ = send.send(Event::Reconfigured(Arc::new(registries)));
            }
            // ClientCommonPacketListenerImpl.handleRequestCookie: the stored
            // cookie (or none) goes straight back.
            play::Clientbound::CookieRequest(q) => {
                let payload = cookies.lock().unwrap().get(&q.key).cloned();
                reply(play::Serverbound::CookieResponse(cfg::CookieResponse {
                    key: q.key,
                    payload,
                }));
            }
            play::Clientbound::StoreCookie(p) => {
                // ModSync over a vanilla cookie: the server's primary
                // delivery path (ClientboundStoreCookiePacket with the key
                // "horizon:modsync") needs no channel registration and no
                // Messenger gate, so the same binary format as the
                // custom-payload channel is parsed here too.
                // RU: ModSync через ванильный cookie — основной путь доставки.
                if p.key == crate::modsync::CHANNEL {
                    log::info!(
                        "ModSync: StoreCookie payload received ({} bytes)",
                        p.payload.len()
                    );
                    match crate::modsync::decode(&p.payload) {
                        Ok(sync) => {
                            let names: Vec<String> = sync
                                .blocks
                                .iter()
                                .take(10)
                                .map(|(ns, path, _)| format!("{ns}:{path}"))
                                .collect();
                            let _ = send.send(Event::Status(format!(
                                "ModSync: received {} content blocks from the server ({})",
                                sync.blocks.len(),
                                names.join(", ")
                            )));
                            let _ = send.send(Event::ModSync(sync.blocks.len()));
                        }
                        Err(e) => {
                            let _ = send
                                .send(Event::Status(format!("ModSync: malformed payload ({e})")));
                        }
                    }
                }
                cookies.lock().unwrap().insert(p.key, p.payload);
            }
            play::Clientbound::ServerLinks(p) => {
                let _ = send.send(Event::ServerLinks(p.links));
            }
            // ClientCommonPacketListenerImpl.handleTransfer: the connection
            // ends and the game connects to the other address.
            play::Clientbound::Transfer(p) => {
                let _ = send.send(Event::Transfer {
                    host: p.host,
                    port: p.port,
                });
                return Ok(tr(lang, "disconnect.transfer"));
            }
            play::Clientbound::Disconnect(d) => return Ok(plain(&d.reason, lang)),
            play::Clientbound::CustomPayload(p) if p.channel == "minecraft:brand" => {
                receive_brand(&p, send);
            }
            play::Clientbound::CustomPayload(p) => {
                log::info!("CustomPayload(diag): channel {} ({} bytes)", p.channel, p.data.len());
            }
            // ModSync in the play state: Paper's plugin messenger delivers
            // server->client plugin messages here, so the same binary format
            // as the configuration-phase channel is parsed again.
            // RU: ModSync в play-фазе — Paper-мессенджер шлёт сюда.
            play::Clientbound::CustomPayload(p) if p.channel == crate::modsync::CHANNEL => {
                log::info!("ModSync: play-state payload received ({} bytes)", p.data.len());
                match crate::modsync::decode(&p.data) {
                    Ok(sync) => {
                        let names: Vec<String> = sync
                            .blocks
                            .iter()
                            .take(10)
                            .map(|(ns, path, _)| format!("{ns}:{path}"))
                            .collect();
                        let _ = send.send(Event::Status(format!(
                            "ModSync: received {} content blocks from the server ({})",
                            sync.blocks.len(),
                            names.join(", ")
                        )));
                        let _ = send.send(Event::ModSync(sync.blocks.len()));
                    }
                    Err(e) => {
                        let _ = send
                            .send(Event::Status(format!("ModSync: malformed payload ({e})")));
                    }
                }
            }
            other => {
                // Paper's NMS serializes common packets (StoreCookie) with the
                // configuration-state id even while the connection is in play:
                // catch that here and route it through cookie handling.
                // RU: Paper шлёт store cookie с configuration-id в play.
                if let play::Clientbound::Unknown(id, payload) = &other {
                    if *id == horizon_protocol::ids::configuration::clientbound::STORE_COOKIE {
                        if let Ok(p) = <cfg::StoreCookie as horizon_protocol::buf::Codec<cfg::StoreCookie>>::read(
                            &mut horizon_protocol::buf::Reader::new(payload),
                        ) {
                            if p.key == crate::modsync::CHANNEL {
                                log::info!("ModSync: config-id StoreCookie received ({} bytes)", p.payload.len());
                                match crate::modsync::decode(&p.payload) {
                                    Ok(sync) => {
                                        let names: Vec<String> = sync.blocks.iter().take(10)
                                            .map(|(ns, path, _)| format!("{ns}:{path}")).collect();
                                        let _ = send.send(Event::Status(format!(
                                            "ModSync: received {} content blocks from the server ({})",
                                            sync.blocks.len(), names.join(", "))));
                                        let _ = send.send(Event::ModSync(sync.blocks.len()));
                                    }
                                    Err(e) => {
                                        let _ = send.send(Event::Status(format!("ModSync: malformed payload ({e})")));
                                    }
                                }
                            }
                            cookies.lock().unwrap().insert(p.key, p.payload);
                            continue;
                        }
                    }
                }
                if send.send(Event::Packet(Box::new(other))).is_err() {
                    return Ok("Disconnected".into());
                }
            }
        }
    }
}

/// The configuration phase: registries, tags and known packs until the
/// server finishes it (ClientConfigurationPacketListenerImpl).
fn configure(
    reader: &mut ConnReader,
    writer: &Mutex<Option<ConnWriter>>,
    configuring: &AtomicBool,
    lang: &Language,
    send: &Sender<Event>,
    cookies: &Cookies,
    resources: &Path,
    probed: Option<i32>,
) -> Result<Dynamic, String> {
    let mut registries = Dynamic::default();
    let reply = |p: cfg::Serverbound| -> Result<(), String> {
        match writer.lock().unwrap().as_mut() {
            Some(w) => w.send(&p.encode()).map_err(|e| e.to_string()),
            None => Err("Disconnected".into()),
        }
    };
    loop {
        let (id, payload) = reader.recv().map_err(|e| e.to_string())?;
        // 767 configuration packet ids differ from 777 from transfer on:
        // 767 has no post_effects, so every id >= 0x0B (transfer) is one less
        // than the 777 id. Remap before decoding with the 777 tables.
        // / RU: id 767 начиная с transfer сдвинуты на один - у 767 нет
        // post_effects. Ремапим перед декодом 777-таблицами.
        let id = if probed == Some(767) && id >= 0x0B { id + 1 } else { id };
        match cfg::Clientbound::decode(id, &payload).map_err(|e| e.to_string())? {
            cfg::Clientbound::KeepAlive(k) => reply(cfg::Serverbound::KeepAlive(k))?,
            cfg::Clientbound::CustomPayload(p) if p.channel == crate::content_identity::CHANNEL => {
                let hash = crate::content_identity::identity(resources)
                    .map_err(|_| crate::content_identity::MISMATCH.to_string())?;
                reply(cfg::Serverbound::CustomPayload(cfg::CustomPayload {
                    channel: crate::content_identity::CHANNEL.into(),
                    data: hash.to_vec(),
                }))?;
            }
            // ModSync: content blocks pushed by a Horizon-aware server. One-way
            // (no reply); a malformed payload is a status message, not a
            // disconnect — the server may keep sending configuration packets.
            cfg::Clientbound::CustomPayload(p) if p.channel == crate::modsync::CHANNEL => {
                match crate::modsync::decode(&p.data) {
                    Ok(payload) => {
                        let names: Vec<String> = payload
                            .blocks
                            .iter()
                            .take(10)
                            .map(|(namespace, path, _)| format!("{namespace}:{path}"))
                            .collect();
                        let _ = send.send(Event::Status(format!(
                            "ModSync: received {} content blocks from the server ({}…)",
                            payload.blocks.len(),
                            names.join(", ")
                        )));
                        let _ = send.send(Event::ModSync(payload.blocks.len()));
                    }
                    Err(e) => {
                        let _ = send.send(Event::Status(format!("ModSync: malformed payload ({e})")));
                    }
                }
            }
            // NeoForge 1.21.x negotiation (ViaHorizonPlus): answer the
            // channel query and the two blocking payloads so the server
            // finishes configuration for a "vanilla" client.
            // / RU: неготовация NeoForge: отвечаем на запрос каналов и
            // два блокирующих payload, чтобы сервер завершил фазу.
            cfg::Clientbound::CustomPayload(p) if p.channel == "neoforge:register" => {
                log::info!(
                    "ViaHorizonPlus: neoforge:register query received ({} bytes, head {:02X?})",
                    p.data.len(),
                    p.data.iter().take(24)
                );
                // Mirror the server's channel query back with the same
                // channels and versions: this is what a matching NeoForge
                // client answers, so the negotiation succeeds.
                // / RU: зеркалим запрос каналов сервера - как клиент
                // с совпадающим набором, чтобы неготовация прошла.
                let mut data = Vec::new();
                // Parse the query: varint mapSize; per entry: proto varint,
                // count varint; per component: id string, version string,
                // bool flowPresent + varint, bool optional.
                let q = &p.data;
                let mut pos = 0usize;
                let map_size = buf_ext::read_varint_at(q, &mut pos).unwrap_or(0);
                buf_ext::write_varint_out(map_size, &mut data);
                for _m in 0..map_size {
                    let proto = buf_ext::read_varint_at(q, &mut pos).unwrap_or(0);
                    buf_ext::write_varint_out(proto, &mut data);
                    let count = buf_ext::read_varint_at(q, &mut pos).unwrap_or(0);
                    buf_ext::write_varint_out(count, &mut data);
                    for _c in 0..count {
                        let id = buf_ext::read_string_at(q, &mut pos).unwrap_or_default();
                        let version = buf_ext::read_string_at(q, &mut pos).unwrap_or_else(|| "1".into());
                        buf_ext::write_varint_out(id.len() as i32, &mut data);
                        data.extend_from_slice(id.as_bytes());
                        buf_ext::write_varint_out(version.len() as i32, &mut data);
                        data.extend_from_slice(version.as_bytes());
                        // flow: bool present + varint ordinal
                        let flow_present = *q.get(pos).unwrap_or(&0);
                        pos += 1;
                        if flow_present != 0 {
                            let _ = buf_ext::read_varint_at(q, &mut pos);
                        }
                        let _optional = *q.get(pos).unwrap_or(&0);
                        pos += 1;
                        // answer with no flow restriction
                        data.push(0x00);
                        data.push(0x01); // optional: true
                    }
                }
                reply(cfg::Serverbound::CustomPayload(cfg::CustomPayload {
                    channel: "neoforge:register".into(),
                    data,
                }))?;
            }
            cfg::Clientbound::CustomPayload(p) if p.channel == "neoforge:known_registry_data_maps" => {
                // Mirror every data map the server listed (mandatory ones
                // included) back as known: the Horizon client learns the
                // maps from the server payload itself, so the check passes.
                // / RU: зеркалим все data maps сервера как известные -
                // клиент узнаёт их из самого payload сервера.
                // Query: varint mapSize; per registry: string registryKey;
                // varint listSize; per entry: string mapId, bool mandatory.
                let q = &p.data;
                let mut pos = 0usize;
                let map_size = buf_ext::read_varint_at(q, &mut pos).unwrap_or(0);
                let mut data = Vec::new();
                buf_ext::write_varint(map_size, &mut data);
                let mut total = 0usize;
                for _m in 0..map_size {
                    let registry = buf_ext::read_string_at(q, &mut pos).unwrap_or_default();
                    let list_size = buf_ext::read_varint_at(q, &mut pos).unwrap_or(0);
                    buf_ext::write_varint_out(registry.len() as i32, &mut data);
                    data.extend_from_slice(registry.as_bytes());
                    buf_ext::write_varint(list_size, &mut data);
                    for _e in 0..list_size {
                        let map_id = buf_ext::read_string_at(q, &mut pos).unwrap_or_default();
                        let _mandatory = *q.get(pos).unwrap_or(&0);
                        pos += 1;
                        buf_ext::write_varint_out(map_id.len() as i32, &mut data);
                        data.extend_from_slice(map_id.as_bytes());
                        total += 1;
                    }
                }
                log::info!(
                    "ViaHorizonPlus: known_registry_data_maps query mirrored ({} registries, {} maps)",
                    map_size,
                    total
                );
                reply(cfg::Serverbound::CustomPayload(cfg::CustomPayload {
                    channel: "neoforge:known_registry_data_maps_reply".into(),
                    data,
                }))?;
            }
            cfg::Clientbound::CustomPayload(p) if p.channel == "bettercombat:config_sync"
                || p.channel == "bettercombat:weapon_registry" =>
            {
                // Better Combat handshake: every config payload is answered
                // with an Ack on `bettercombat:ack` whose code is the
                // channel that was received - that is what completes the
                // mod's configuration tasks on the server.
                // / RU: рукопожатие Better Combat: на каждый config payload
                // отвечаем Ack с кодом = имя канала - так завершаются
                // конфиг-задачи мода на сервере.
                // The ack code is the task key, not the channel name:
                // config_sync answers with "bettercombat:config".
                // / RU: код подтверждения - ключ задачи, а не имя канала.
                let code = if p.channel == "bettercombat:config_sync" {
                    "bettercombat:config".to_string()
                } else {
                    p.channel.clone()
                };
                let mut data = Vec::new();
                let bytes = code.as_bytes();
                buf_ext::write_varint_out(bytes.len() as i32, &mut data);
                data.extend_from_slice(bytes);
                reply(cfg::Serverbound::CustomPayload(cfg::CustomPayload {
                    channel: "bettercombat:ack".into(),
                    data,
                }))?;
                log::info!("ViaHorizonPlus: better combat ack sent ({})", code);
            }
            cfg::Clientbound::CustomPayload(p) if p.channel == "neoforge:feature_flags" => {
                // mechanics-mod negotiation: echo the feature flags back as
                // the acknowledgement a real modded client sends.
                // / RU: переговоры мода mechanics - эхо флагов как
                // подтверждение, которое шлёт настоящий модовый клиент.
                // NeoForge 21.1.247+ CheckFeatureFlags task: the server
                // sends its modded feature flags and waits for the empty
                // FeatureFlagAcknowledgePayload on the ack channel.
                // / RU: задача CheckFeatureFlags - сервер шлёт свои модовые
                // флаги и ждёт пустой ack на канале feature_flags_ack.
                reply(cfg::Serverbound::CustomPayload(cfg::CustomPayload {
                    channel: "neoforge:feature_flags_ack".into(),
                    data: Vec::new(),
                }))?;
                log::info!(
                    "ViaHorizonPlus: feature flags acknowledged ({} flags payload bytes)",
                    p.data.len()
                );
            }
            cfg::Clientbound::CustomPayload(p) if p.channel == "neoforge:frozen_registry_sync_completed" => {
                // The real NeoForge client applies the frozen registry
                // snapshots and answers with the same empty payload to
                // finish the SyncRegistries task. Horizon keeps the data for
                // ModSync but must answer or the server stalls.
                // / RU: настоящий клиент применяет снапшоты реестров и
                // отвечает тем же пустым payload - иначе сервер зависает.
                let _ = &p;
                reply(cfg::Serverbound::CustomPayload(cfg::CustomPayload {
                    channel: "neoforge:frozen_registry_sync_completed".into(),
                    data: Vec::new(),
                }))?;
                log::info!("ViaHorizonPlus: frozen registry sync acknowledged");
            }
            cfg::Clientbound::CustomPayload(p) if p.channel == "neoforge:extensible_enum_data" => {
                reply(cfg::Serverbound::CustomPayload(cfg::CustomPayload {
                    channel: "neoforge:extensible_enum_ack".into(),
                    data: Vec::new(),
                }))?;
            }
            cfg::Clientbound::Ping(p) if p.id == 0 => {
                reply(cfg::Serverbound::Pong(p))?;
            }
            cfg::Clientbound::CustomPayload(p) => {
                log::info!(
                    "ViaHorizonPlus: custom payload channel {} ({} bytes) passthrough",
                    p.channel,
                    p.data.len()
                );
                receive_brand(&p, send);
            }
            cfg::Clientbound::Ping(p) => reply(cfg::Serverbound::Pong(p))?,
            // Know no packs: the server then sends every registry entry in full.
            cfg::Clientbound::SelectKnownPacks(_) => {
                reply(cfg::Serverbound::SelectKnownPacks(cfg::SelectKnownPacks {
                    packs: vec![],
                }))?
            }
            cfg::Clientbound::RegistryData(r) => registries.add(r),
            cfg::Clientbound::UpdateTags(t) => registries.add_tags(&t),
            cfg::Clientbound::CodeOfConduct(c) => {
                let _ = send.send(Event::CodeOfConduct(c.text));
            }
            // The game answers: it may prompt the player first.
            cfg::Clientbound::ResourcePackPush(p) => {
                let _ = send.send(Event::PackPush(p));
            }
            cfg::Clientbound::ResourcePackPop(p) => {
                let _ = send.send(Event::PackPop(p.id));
            }
            // The game shows it; the answer goes back from there.
            cfg::Clientbound::ShowDialog(d) => {
                let _ = send.send(Event::ShowDialog(d.dialog));
            }
            cfg::Clientbound::ClearDialog(_) => {
                let _ = send.send(Event::ClearDialog);
            }
            cfg::Clientbound::CookieRequest(q) => {
                let payload = cookies.lock().unwrap().get(&q.key).cloned();
                reply(cfg::Serverbound::CookieResponse(cfg::CookieResponse {
                    key: q.key,
                    payload,
                }))?
            }
            cfg::Clientbound::StoreCookie(p) => {
                cookies.lock().unwrap().insert(p.key, p.payload);
            }
            cfg::Clientbound::ServerLinks(p) => {
                let _ = send.send(Event::ServerLinks(p.links));
            }
            cfg::Clientbound::Transfer(p) => {
                let _ = send.send(Event::Transfer {
                    host: p.host,
                    port: p.port,
                });
                return Err(tr(lang, "disconnect.transfer"));
            }
            cfg::Clientbound::Disconnect(d) => return Err(plain(&d.reason, lang)),
            cfg::Clientbound::FinishConfiguration(_) => {
                let mut w = writer.lock().unwrap();
                let w = w.as_mut().ok_or("Disconnected")?;
                w.send(&cfg::Serverbound::FinishConfiguration(Empty).encode())
                    .map_err(|e| e.to_string())?;
                configuring.store(false, Ordering::Relaxed);
                return Ok(registries);
            }
            _ => {}
        }
    }
}

fn receive_brand(payload: &cfg::CustomPayload, send: &Sender<Event>) {
    if payload.channel == "minecraft:brand"
        && let Ok(brand) = horizon_protocol::buf::Reader::new(&payload.data).string(32767)
    {
        let _ = send.send(Event::Brand(brand));
    } else if payload.channel == horizon_protocol::quiet::CHANNEL {
        let _ = send.send(Event::QuietCommands);
    }
}

/// The minecraft:brand payload: a string naming the client.
fn brand() -> Vec<u8> {
    let mut w = horizon_protocol::buf::Writer::new();
    w.string("horizon");
    w.data
}

/// Flatten a JSON chat component (login disconnects are JSON).
pub fn json_text(v: &serde_json::Value, lang: &Language) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => a.iter().map(|e| json_text(e, lang)).collect(),
        serde_json::Value::Object(o) => {
            let mut out = if let Some(t) = o.get("text").and_then(|t| t.as_str()) {
                t.to_owned()
            } else if let Some(key) = o.get("translate").and_then(|t| t.as_str()) {
                let args: Vec<String> = o
                    .get("with")
                    .and_then(|w| w.as_array())
                    .map(|a| a.iter().map(|e| json_text(e, lang)).collect())
                    .unwrap_or_default();
                let pattern = lang
                    .get(key)
                    .cloned()
                    .or_else(|| {
                        o.get("fallback")
                            .and_then(|f| f.as_str())
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| key.to_owned());
                horizon_protocol::text::format_java(&pattern, &args)
            } else {
                String::new()
            };
            if let Some(extra) = o.get("extra").and_then(|e| e.as_array()) {
                for e in extra {
                    out.push_str(&json_text(e, lang));
                }
            }
            out
        }
        other => other.to_string(),
    }
}

/// The language a session resolves server-sent components with: the selected
/// one as `lang` loaded it (`en_us` under it, packs included), so a menu title
/// the server sends as a key follows the language screen. A process with no
/// language set (a dedicated server's log) reads en_us.json.
pub fn load_language(resources: &std::path::Path) -> Language {
    if let Some(lang) = crate::lang::current() {
        return lang.map().clone();
    }
    std::fs::read(resources.join("assets/minecraft/lang/en_us.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// ViaHorizonPlus step 1: read the server's version through a status ping
/// (horizon-via's probe). `None` when the server speaks the client's own
/// 26.3, when the probe cannot reach or parse the server (an unreachable
/// or legacy-ping server must never block the join), or when the server
/// reports a version the probe cannot read.
///
/// EN: the answer is used for the "server runs <name> (protocol <n>)"
/// status message and stored on [`Session::probed`]; the translation layer
/// (step 2) will consume it later.
///
/// RU: пробы версии сервера; None — совпадение, недоступность или
/// нечитаемый ответ. Результат идёт в сообщение и `Session::probed`.
fn probe_server_version(address: &str) -> Option<(i32, String)> {
    let server = horizon_via::probe::probe_server(address, 3).ok()?;
    if server.protocol == PROTOCOL_VERSION {
        return None;
    }
    Some((server.protocol, server.version_name))
}

/// Ask a server for its status (ServerStatusPinger): MOTD, players and the
/// round trip time in milliseconds.
pub fn ping(address: &str, timeout: Duration) -> Result<(serde_json::Value, u128), String> {
    use horizon_protocol::packets::status;
    let (host, port) = parse_address(address).ok_or("Invalid address")?;
    let mut c = Connection::connect_server(&host, port, timeout).map_err(|e| e.to_string())?;
    c.reader
        .set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    let e = |e: horizon_protocol::Error| e.to_string();
    c.send(
        &handshake::Serverbound::Intention(handshake::Intention {
            protocol: PROTOCOL_VERSION,
            host: host.clone(),
            port,
            intent: handshake::STATUS,
        })
        .encode(),
    )
    .map_err(e)?;
    c.send(&status::Serverbound::StatusRequest(Empty).encode())
        .map_err(e)?;
    let (id, payload) = c.recv().map_err(e)?;
    let json = match status::Clientbound::decode(id, &payload).map_err(e)? {
        status::Clientbound::StatusResponse(s) => {
            serde_json::from_str(&s.json).map_err(|e| e.to_string())?
        }
        _ => return Err("unexpected status reply".into()),
    };
    let start = std::time::Instant::now();
    c.send(&status::Serverbound::PingRequest(status::Ping { time: 1 }).encode())
        .map_err(e)?;
    let _ = c.recv().map_err(e)?;
    Ok((json, start.elapsed().as_millis()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_answers_the_content_identity_challenge() {
        let root =
            std::env::temp_dir().join(format!("horizon-content-identity-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("facts.json"),
            br#"{"states":[[0,0,0,0,"minecraft:stone"]]}"#,
        )
        .unwrap();
        let expected = crate::content_identity::identity(&root).unwrap();
        let (client, server) = horizon_protocol::connection::Pipe::pair();
        let (mut reader, writer) = Connection::local(client).split();
        let writer = Mutex::new(Some(writer));
        let thread = std::thread::spawn(move || {
            let (send, _events) = channel();
            configure(
                &mut reader,
                &writer,
                &AtomicBool::new(true),
                &Language::new(),
                &send,
                &Cookies::default(),
                &root,
                None,
            )
        });
        let mut server = Connection::local(server);
        server
            .send(
                &cfg::Clientbound::CustomPayload(cfg::CustomPayload {
                    channel: crate::content_identity::CHANNEL.into(),
                    data: vec![],
                })
                .encode(),
            )
            .unwrap();
        server
            .send(&cfg::Clientbound::Ping(cfg::Ping { id: 17 }).encode())
            .unwrap();
        let (id, body) = server.recv().unwrap();
        assert_eq!(
            cfg::Serverbound::decode(id, &body).unwrap(),
            cfg::Serverbound::CustomPayload(cfg::CustomPayload {
                channel: crate::content_identity::CHANNEL.into(),
                data: expected.to_vec(),
            })
        );
        let (id, body) = server.recv().unwrap();
        assert_eq!(
            cfg::Serverbound::decode(id, &body).unwrap(),
            cfg::Serverbound::Pong(cfg::Ping { id: 17 })
        );
        server
            .send(&cfg::Clientbound::FinishConfiguration(Empty).encode())
            .unwrap();
        let (id, body) = server.recv().unwrap();
        assert_eq!(
            cfg::Serverbound::decode(id, &body).unwrap(),
            cfg::Serverbound::FinishConfiguration(Empty)
        );
        assert!(thread.join().unwrap().is_ok());
        std::fs::remove_dir_all(
            std::env::temp_dir().join(format!("horizon-content-identity-{}", std::process::id())),
        )
        .unwrap();
    }

    #[test]
    fn modsync_payloads_are_reported_and_malformed_ones_do_not_disconnect() {
        let (client, server) = horizon_protocol::connection::Pipe::pair();
        let (mut reader, writer) = Connection::local(client).split();
        let writer = Mutex::new(Some(writer));
        let (send, events) = channel();
        let thread = std::thread::spawn(move || {
            configure(
                &mut reader,
                &writer,
                &AtomicBool::new(true),
                &Language::new(),
                &send,
                &Cookies::default(),
                Path::new("."),
                None,
            )
        });
        let mut server = Connection::local(server);
        // One valid block: "HMS1", 1 entry, namespace "demo", path "ruby_ore",
        // JSON {"template":"minecraft:stone"}.
        let mut good = horizon_protocol::buf::Writer::new();
        good.bytes(b"HMS1");
        good.varint(1);
        good.string("demo");
        good.string("ruby_ore");
        good.byte_array(br#"{"template":"minecraft:stone"}"#);
        server
            .send(&cfg::Clientbound::CustomPayload(cfg::CustomPayload {
                channel: crate::modsync::CHANNEL.into(),
                data: good.data,
            })
            .encode())
            .unwrap();
        // A malformed one right after: bad magic.
        server
            .send(&cfg::Clientbound::CustomPayload(cfg::CustomPayload {
                channel: crate::modsync::CHANNEL.into(),
                data: b"BAD!".to_vec(),
            })
            .encode())
            .unwrap();
        server
            .send(&cfg::Clientbound::FinishConfiguration(Empty).encode())
            .unwrap();
        let (id, body) = server.recv().unwrap();
        assert_eq!(
            cfg::Serverbound::decode(id, &body).unwrap(),
            cfg::Serverbound::FinishConfiguration(Empty)
        );
        assert!(thread.join().unwrap().is_ok());
        let mut statuses = Vec::new();
        let mut counts = Vec::new();
        for event in events.try_iter() {
            match event {
                Event::Status(s) => statuses.push(s),
                Event::ModSync(n) => counts.push(n),
                _ => {}
            }
        }
        assert_eq!(counts, vec![1]);
        assert!(
            statuses.iter().any(|s| s.starts_with("ModSync: received 1 content blocks")
                && s.contains("demo:ruby_ore"))
        );
        assert!(statuses.iter().any(|s| s.starts_with("ModSync: malformed payload")));
    }

    #[test]
    fn disconnect_and_drop_do_not_wait_for_a_stalled_reader() {
        // A reader can remain stalled after Windows socket shutdown. It must
        // not delay either an explicit leave or Session's Drop on the UI thread.
        for explicit_disconnect in [true, false] {
            let (client, _server) = horizon_protocol::connection::Pipe::pair();
            let (_reader, writer) = Connection::local(client).split();
            let writer = Arc::new(Mutex::new(Some(writer)));
            let (_send, events) = channel();
            let (release, stalled) = channel();
            let (finished, completed) = channel();
            let thread = std::thread::spawn(move || {
                let _ = stalled.recv_timeout(std::time::Duration::from_secs(3));
                let _ = finished.send(());
            });
            let mut session = Session {
                events,
                writer: writer.clone(),
                configuring: Arc::new(AtomicBool::new(false)),
                thread: Some(thread),
                address: "local".into(),
                name: "TestPlayer".into(),
                probed: Arc::new(Mutex::new(None)),
                counters: Arc::new(Mutex::new(None)),
                cookies: Cookies::default(),
                debug: false,
            };
            let started = std::time::Instant::now();
            if explicit_disconnect {
                session.disconnect();
                session.disconnect(); // Repeated cleanup is harmless.
            }
            drop(session);
            let elapsed = started.elapsed();
            let reader_still_pending = completed.try_recv().is_err();
            // Always release the synthetic reader before asserting, even if
            // a future regression restores the blocking join.
            let _ = release.send(());
            if reader_still_pending {
                completed
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap();
            }
            assert!(
                writer.lock().unwrap().is_none(),
                "connection was not closed"
            );
            assert!(
                reader_still_pending,
                "disconnect waited for the stalled reader"
            );
            assert!(
                elapsed < std::time::Duration::from_secs(1),
                "disconnect took {elapsed:?}"
            );
        }
    }

    #[test]
    fn pings_wait_behind_teleports_but_keepalives_do_not_wait_for_the_game() {
        let (client, server) = horizon_protocol::connection::Pipe::pair();
        let (mut reader, writer) = Connection::local(client).split();
        let writer = Arc::new(Mutex::new(Some(writer)));
        let (send, events) = channel();
        let thread_writer = writer.clone();
        let thread = std::thread::spawn(move || {
            receive_play(
                &mut reader,
                &thread_writer,
                &AtomicBool::new(false),
                &Language::new(),
                &send,
                &Cookies::default(),
                Path::new("."),
                None,
            )
        });
        let mut server = Connection::local(server);
        let teleport = play::Clientbound::PlayerPosition(play::PlayerPosition {
            teleport: 42,
            position: [10.0, 64.0, 10.0],
            delta: [0.0; 3],
            yaw: 90.0,
            pitch: 0.0,
            relatives: 0,
        });
        server.send(&teleport.encode()).unwrap();
        server
            .send(&play::Clientbound::Ping(cfg::Ping { id: 7 }).encode())
            .unwrap();
        // A reply to this proves the receive loop has passed the ping. The
        // old implementation instead replies with Pong before this keepalive.
        server
            .send(&play::Clientbound::KeepAlive(cfg::KeepAlive { id: 9 }).encode())
            .unwrap();
        let (id, body) = server.recv().unwrap();
        assert_eq!(
            play::Serverbound::decode(id, &body).unwrap(),
            play::Serverbound::KeepAlive(cfg::KeepAlive { id: 9 })
        );
        let timeout = Duration::from_secs(5);
        assert!(
            matches!(events.recv_timeout(timeout).unwrap(), Event::Packet(p) if *p == teleport)
        );
        assert!(
            matches!(events.recv_timeout(timeout).unwrap(), Event::Packet(p) if *p == play::Clientbound::Ping(cfg::Ping { id: 7 }))
        );
        writer.lock().unwrap().take().unwrap().shutdown();
        assert!(thread.join().unwrap().is_err());
    }

    #[test]
    fn play_packets_wait_out_a_reconfiguration() {
        let (client, server) = horizon_protocol::connection::Pipe::pair();
        let (mut reader, writer) = Connection::local(client).split();
        let (send, events) = channel();
        let session = Session {
            events,
            writer: Arc::new(Mutex::new(Some(writer))),
            configuring: Arc::new(AtomicBool::new(false)),
            thread: None,
            address: "local".into(),
            name: "Player".into(),
            probed: Arc::new(Mutex::new(None)),
            counters: Arc::new(Mutex::new(None)),
            cookies: Cookies::default(),
            debug: false,
        };
        let (thread_writer, thread_configuring) =
            (session.writer.clone(), session.configuring.clone());
        let thread = std::thread::spawn(move || {
            receive_play(
                &mut reader,
                &thread_writer,
                &thread_configuring,
                &Language::new(),
                &send,
                &Cookies::default(),
                Path::new("."),
            )
        });
        let mut server = Connection::local(server);
        let tick_end = play::Serverbound::ClientTickEnd(Empty);
        server
            .send(&play::Clientbound::StartConfiguration(Empty).encode())
            .unwrap();
        let (id, body) = server.recv().unwrap();
        assert_eq!(
            play::Serverbound::decode(id, &body).unwrap(),
            play::Serverbound::ConfigurationAcknowledged(Empty)
        );
        // The game thread still ticks the old level: nothing of it goes out.
        session.send(tick_end.clone());
        server
            .send(&cfg::Clientbound::FinishConfiguration(Empty).encode())
            .unwrap();
        let (id, body) = server.recv().unwrap();
        assert_eq!(
            cfg::Serverbound::decode(id, &body).unwrap(),
            cfg::Serverbound::FinishConfiguration(Empty)
        );
        // Back in play, the game's packets flow again.
        session.send(tick_end.clone());
        let (id, body) = server.recv().unwrap();
        assert_eq!(play::Serverbound::decode(id, &body).unwrap(), tick_end);
        drop(session);
        assert!(thread.join().unwrap().is_err());
    }

    #[test]
    fn play_answers_cookie_requests_from_stored_cookies_and_follows_transfers() {
        let (client, server) = horizon_protocol::connection::Pipe::pair();
        let (mut reader, writer) = Connection::local(client).split();
        let writer = Arc::new(Mutex::new(Some(writer)));
        let (send, events) = channel();
        let cookies = Cookies::default();
        let (thread_writer, thread_cookies) = (writer.clone(), cookies.clone());
        let thread = std::thread::spawn(move || {
            receive_play(
                &mut reader,
                &thread_writer,
                &AtomicBool::new(false),
                &Language::new(),
                &send,
                &thread_cookies,
                Path::new("."),
            )
        });
        let mut server = Connection::local(server);
        let ask = |server: &mut Connection, key: &str| {
            let request = play::Clientbound::CookieRequest(cfg::CookieRequest { key: key.into() });
            server.send(&request.encode()).unwrap();
            let (id, body) = server.recv().unwrap();
            play::Serverbound::decode(id, &body).unwrap()
        };
        // handleRequestCookie answers an unknown key with no payload.
        assert_eq!(
            ask(&mut server, "proxy:session"),
            play::Serverbound::CookieResponse(cfg::CookieResponse {
                key: "proxy:session".into(),
                payload: None,
            })
        );
        let store = play::Clientbound::StoreCookie(cfg::StoreCookie {
            key: "proxy:session".into(),
            payload: vec![1, 2, 3],
        });
        server.send(&store.encode()).unwrap();
        assert_eq!(
            ask(&mut server, "proxy:session"),
            play::Serverbound::CookieResponse(cfg::CookieResponse {
                key: "proxy:session".into(),
                payload: Some(vec![1, 2, 3]),
            })
        );
        let transfer = play::Clientbound::Transfer(cfg::Transfer {
            host: "example.net".into(),
            port: 25566,
        });
        server.send(&transfer.encode()).unwrap();
        assert!(thread.join().unwrap().is_ok());
        assert!(matches!(
            events.try_iter().last(),
            Some(Event::Transfer { host, port: 25566 }) if host == "example.net"
        ));
        // The stored cookie stays for the next connection (TransferState).
        assert_eq!(
            cookies.lock().unwrap().get("proxy:session"),
            Some(&vec![1, 2, 3])
        );
    }

    #[test]
    fn server_brand_payload_is_decoded_and_invalid_or_unrelated_payloads_are_ignored() {
        let (send, events) = channel();
        let mut writer = horizon_protocol::buf::Writer::new();
        writer.string("vanilla");
        let mut payload = cfg::CustomPayload {
            channel: "minecraft:brand".into(),
            data: writer.data,
        };
        receive_brand(&payload, &send);
        assert!(matches!(events.try_recv(), Ok(Event::Brand(brand)) if brand == "vanilla"));
        payload.channel = "example:other".into();
        receive_brand(&payload, &send);
        assert!(events.try_recv().is_err());
        payload.channel = "minecraft:brand".into();
        payload.data = vec![0xff];
        receive_brand(&payload, &send);
        assert!(events.try_recv().is_err());
    }
}

/// VarInt writer for hand-built NeoForge payload bodies.
/// / RU: запись VarInt для самодельных payload NeoForge.
mod buf_ext {
    pub fn write_varint(value: i32, out: &mut Vec<u8>) {
        let mut v = value as u32;
        loop {
            let byte = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
    }

    pub fn read_varint_at(buf: &[u8], pos: &mut usize) -> Option<i32> {
        let mut value: i32 = 0;
        let mut shift = 0;
        loop {
            let b = *buf.get(*pos)?;
            *pos += 1;
            value |= ((b & 0x7F) as i32) << shift;
            shift += 7;
            if b & 0x80 == 0 {
                return Some(value);
            }
        }
    }

    pub fn read_string_at(buf: &[u8], pos: &mut usize) -> Option<String> {
        let len = read_varint_at(buf, pos)? as usize;
        let s = String::from_utf8_lossy(buf.get(*pos..(*pos + len))?).to_string();
        *pos += len;
        Some(s)
    }

    pub fn write_varint_out(value: i32, out: &mut Vec<u8>) {
        write_varint(value, out);
    }
}

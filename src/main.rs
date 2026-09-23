use anyhow::Result;
use matrix_sdk::{
    authentication::matrix::MatrixSession,
    config::SyncSettings,
    room::Room,
    ruma::{
        events::room::message::SyncRoomMessageEvent,
        RoomId,
    },
    Client,
};
use matrix_sdk_crypto::CollectStrategy;
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

// ============================================================
// CONFIGURATION
// ============================================================

/// Parses a required, comma-separated list of room IDs from the
/// named environment variable. Trims whitespace, drops empty
/// entries, validates each with `RoomId::parse`, and de-duplicates.
/// Bails with a clear error if the variable is unset or contains
/// no valid room IDs.
fn parse_room_list(env_var: &str) -> Result<Vec<String>> {
    let raw = std::env::var(env_var).map_err(|_| {
        anyhow::anyhow!("{} must be set in .env", env_var)
    })?;

    let mut rooms: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for room_string in raw.split(',') {
        let room_string = room_string.trim();

        if room_string.is_empty() {
            continue;
        }

        let room_id = RoomId::parse(room_string)?;
        let normalized = room_id.to_string();

        if seen.insert(normalized.clone()) {
            rooms.push(normalized);
        }
    }

    if rooms.is_empty() {
        anyhow::bail!(
            "{} must contain at least one valid room ID",
            env_var
        );
    }

    Ok(rooms)
}

struct Config {
    homeserver: String,
    username: String,
    password: String,

    recovery_key: String,
    passphrase: String,

    target_rooms: Vec<String>,
    trigger_rooms: Vec<String>,

    session_file: PathBuf,
    key_export_file: PathBuf,
    crypto_store_path: PathBuf,
    diagnostic_key_file: PathBuf,

    welcome_trigger: String,
    debounce_seconds: u64,
}

impl Config {
    fn from_env() -> Result<Self> {
        let homeserver = std::env::var("MATRIX_HOMESERVER")
            .unwrap_or_else(|_| "https://matrix.org".to_string());

        let username = std::env::var("MATRIX_USERNAME")?;
        let password = std::env::var("MATRIX_PASSWORD")?;

        let recovery_key = std::env::var("BOT_RECOVERY_KEY")
            .unwrap_or_default();

        let passphrase =
            std::env::var("KEY_EXPORT_PASSPHRASE")
                .unwrap_or_default();

        // ----------------------------------------------------
        // TARGET ROOMS — where invites are sent.
        //
        // Supports comma-separated room IDs:
        //
        // SPACE_CHILD_ROOMS=!room1:matrix.org,!room2:matrix.org
        // ----------------------------------------------------

        let target_rooms =
            parse_room_list("SPACE_CHILD_ROOMS")?;

        // ----------------------------------------------------
        // TRIGGER ROOMS — where !welcome is accepted from.
        //
        // Required. Without this allow-list, ANY room the bot
        // is a member of would accept the trigger phrase, which
        // means anyone who can message the bot anywhere could
        // self-serve an invite (and full historical decryption
        // keys, since share-on-invite is enabled) into every
        // target room. Same comma-separated format as above:
        //
        // TRIGGER_ROOMS=!lobby:matrix.org
        // ----------------------------------------------------

        let trigger_rooms =
            parse_room_list("TRIGGER_ROOMS")?;

        let session_file = PathBuf::from(
            std::env::var("SESSION_FILE")
                .unwrap_or_else(|_| "./session.json".to_string()),
        );

        let key_export_file = PathBuf::from(
            std::env::var("KEY_EXPORT_FILE")
                .unwrap_or_else(|_| {
                    "./element-keys-shareable.txt".to_string()
                }),
        );

        let crypto_store_path = PathBuf::from(
            std::env::var("CRYPTO_STORE_PATH")
                .unwrap_or_else(|_| "./bot-store".to_string()),
        );

        let diagnostic_key_file = PathBuf::from(
            std::env::var("DIAGNOSTIC_KEY_FILE")
                .unwrap_or_else(|_| {
                    "./diagnostic-keys.txt".to_string()
                }),
        );

        let welcome_trigger =
            std::env::var("WELCOME_TRIGGER")
                .unwrap_or_else(|_| "!welcome".to_string());

        let debounce_seconds =
            std::env::var("WELCOME_DEBOUNCE_SECONDS")
                .unwrap_or_else(|_| "30".to_string())
                .parse::<u64>()?;

        Ok(Self {
            homeserver,
            username,
            password,
            recovery_key,
            passphrase,
            target_rooms,
            trigger_rooms,
            session_file,
            key_export_file,
            crypto_store_path,
            diagnostic_key_file,
            welcome_trigger,
            debounce_seconds,
        })
    }
}

// ============================================================
// MAIN
// ============================================================

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    dotenvy::dotenv().ok();

    let config = Config::from_env()?;

    std::fs::create_dir_all(&config.crypto_store_path)?;

    println!();
    println!("==========================================");
    println!("MATRIX WELCOME BOT");
    println!("Matrix Rust SDK 0.19");
    println!("==========================================");
    println!("Homeserver: {}", config.homeserver);
    println!(
        "Crypto store: {}",
        config.crypto_store_path.display()
    );
    println!(
        "Key export: {}",
        config.key_export_file.display()
    );
    println!(
        "Diagnostic export: {}",
        config.diagnostic_key_file.display()
    );
    println!(
        "Welcome trigger: {}",
        config.welcome_trigger
    );
    println!(
        "Debounce: {} seconds",
        config.debounce_seconds
    );
    println!(
        "Target rooms: {}",
        config.target_rooms.len()
    );

    for room in &config.target_rooms {
        println!("  - {}", room);
    }

    println!(
        "Trigger allowed from: {}",
        config.trigger_rooms.len()
    );

    for room in &config.trigger_rooms {
        println!("  - {}", room);
    }

    println!("==========================================");
    println!();

    // --------------------------------------------------------
    // CREATE CLIENT
    // --------------------------------------------------------

    let client = Client::builder()
        .homeserver_url(&config.homeserver)
        .sqlite_store(&config.crypto_store_path, None)
        .with_enable_share_history_on_invite(true)
        .with_room_key_recipient_strategy(
            CollectStrategy::IdentityBasedStrategy,
        )
        .build()
        .await?;

    // --------------------------------------------------------
    // LOGIN — RESTORE EXISTING SESSION, OR LOG IN AS A NEW DEVICE
    // --------------------------------------------------------

    login_or_restore(
        &client,
        &config.username,
        &config.password,
        &config.session_file,
    )
    .await?;

    println!();
    println!("Logged in as the bot device.");
    println!("Device ID: {:?}", client.device_id());
    println!();

    // --------------------------------------------------------
    // RECOVER CROSS-SIGNING IDENTITY
    // --------------------------------------------------------

    if !config.recovery_key.is_empty() {
        println!("Recovering cross-signing identity...");

        match client
            .encryption()
            .recovery()
            .recover(&config.recovery_key)
            .await
        {
            Ok(_) => {
                println!("Cross-signing identity recovered.");
            }

            Err(error) => {
                println!(
                    "WARNING: Recovery failed: {:#}",
                    error
                );
            }
        }
    } else {
        println!(
            "BOT_RECOVERY_KEY is empty. \
             Skipping cross-signing recovery."
        );
    }

    let status =
        client.encryption().cross_signing_status().await;

    println!(
        "Cross-signing status: {:?}",
        status
    );

    // --------------------------------------------------------
    // IMPORT HISTORICAL ROOM KEYS
    // --------------------------------------------------------

    import_element_keys(
        &client,
        &config.passphrase,
        &config.key_export_file,
    )
    .await?;

    // --------------------------------------------------------
    // DIAGNOSTIC EXPORT
    // --------------------------------------------------------

    diagnose_target_rooms(
        &client,
        &config.target_rooms,
        &config.diagnostic_key_file,
    )
    .await?;

    // --------------------------------------------------------
    // EVENT HANDLER
    // --------------------------------------------------------

    setup_event_handler(&client, &config).await;

    println!();
    println!(
        "Waiting for {}...",
        config.welcome_trigger
    );
    println!();

    // --------------------------------------------------------
    // START SYNC
    // --------------------------------------------------------

    client.sync(SyncSettings::default()).await?;

    Ok(())
}

// ============================================================
// LOGIN / SESSION RESTORE
// ============================================================

/// Restores a previously saved session if one exists, otherwise
/// logs in fresh and creates a new device.
///
/// IMPORTANT: the on-disk crypto store (bot-store) is tied to a
/// specific device ID. If you log in fresh every restart instead
/// of restoring, each run mints a NEW device, and the SDK will
/// refuse to open the OLD device's crypto store for it — you'll
/// see an error like:
///
///   "the account in the store doesn't match the account in the
///    constructor: expected @user:server:OLDDEVICE, got
///    @user:server:NEWDEVICE"
///
/// If you genuinely want to start over as a brand new device,
/// delete BOTH the session file AND the crypto store directory
/// together — deleting only one of the two causes this mismatch.
async fn login_or_restore(
    client: &Client,
    username: &str,
    password: &str,
    session_path: &PathBuf,
) -> Result<()> {
    if session_path.exists() {
        println!(
            "Existing session found at {} — restoring...",
            session_path.display()
        );

        let session_file = File::open(session_path)?;
        let session: MatrixSession =
            serde_json::from_reader(session_file)?;

        client.restore_session(session).await?;

        println!("Session restored.");

        return Ok(());
    }

    println!(
        "No existing session found. Logging in as {}...",
        username
    );
    println!("A new Matrix device will be created.");

    let response = client
        .matrix_auth()
        .login_username(username, password)
        .send()
        .await?;

    let session: MatrixSession = (&response).into();

    let session_file = File::create(session_path)?;

    serde_json::to_writer_pretty(
        session_file,
        &session,
    )?;

    println!("Login successful.");
    println!(
        "New session saved to {}",
        session_path.display()
    );

    Ok(())
}

// ============================================================
// IMPORT ELEMENT ROOM KEYS
// ============================================================

async fn import_element_keys(
    client: &Client,
    passphrase: &str,
    key_export_path: &PathBuf,
) -> Result<()> {
    let path = key_export_path.clone();

    if !path.exists() {
        println!(
            "No Element key export found at {}",
            path.display()
        );

        return Ok(());
    }

    if passphrase.is_empty() {
        println!(
            "KEY_EXPORT_PASSPHRASE is empty."
        );
        println!(
            "Skipping Element room-key import."
        );

        return Ok(());
    }

    println!();
    println!("==========================================");
    println!("IMPORTING HISTORICAL ROOM KEYS");
    println!("==========================================");
    println!("File: {}", path.display());

    let encryption = client.encryption();

    match encryption
        .import_room_keys(path, passphrase)
        .await
    {
        Ok(result) => {
            println!();
            println!("Room-key import complete.");

            println!(
                "Total keys in export : {}",
                result.total_count
            );

            println!(
                "Keys imported        : {}",
                result.imported_count
            );
        }

        Err(error) => {
            eprintln!();
            eprintln!("ROOM-KEY IMPORT FAILED:");
            eprintln!("{:#}", error);
            eprintln!();

            // Continue running so the failure can be diagnosed.
        }
    }

    println!("==========================================");
    println!();

    Ok(())
}

// ============================================================
// DIAGNOSTIC EXPORT
// ============================================================

async fn diagnose_target_rooms(
    client: &Client,
    target_rooms: &[String],
    diagnostic_path: &PathBuf,
) -> Result<()> {
    let encryption = client.encryption();

    for target_room in target_rooms {
        println!();
        println!(
            "Diagnosing target room: {}",
            target_room
        );

        let diagnostic_path = diagnostic_path.clone();

        let keys = encryption
            .export_room_keys(
                diagnostic_path,
                "",
                |session| {
                    session.room_id().to_string()
                        == *target_room
                },
            )
            .await;

        match keys {
            Ok(_) => {
                println!(
                    "Diagnostic export completed for {}.",
                    target_room
                );
            }

            Err(error) => {
                println!(
                    "Diagnostic export failed for {}: {:#}",
                    target_room,
                    error
                );
            }
        }
    }

    Ok(())
}

// ============================================================
// EVENT HANDLER
// ============================================================

async fn setup_event_handler(
    client: &Client,
    config: &Config,
) {
    let debounce: Arc<Mutex<HashMap<String, Instant>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let debounce_for_handler = debounce.clone();

    let target_rooms = config.target_rooms.clone();
    let trigger_rooms = config.trigger_rooms.clone();

    let welcome_trigger =
        config.welcome_trigger.clone();

    let debounce_seconds =
        config.debounce_seconds;

    let diagnostic_key_file =
        config.diagnostic_key_file.clone();

    client.add_event_handler(
        move |event: SyncRoomMessageEvent,
              room: Room,
              client: Client| {
            let debounce =
                debounce_for_handler.clone();

            let target_rooms =
                target_rooms.clone();

            let trigger_rooms =
                trigger_rooms.clone();

            let welcome_trigger =
                welcome_trigger.clone();

            let diagnostic_key_file =
                diagnostic_key_file.clone();

            async move {
                // ------------------------------------------------
                // ENFORCE TRIGGER-ROOM ALLOW-LIST
                //
                // Bail out before even looking at the message body
                // if this event didn't come from a room we've
                // explicitly allow-listed via TRIGGER_ROOMS.
                // Otherwise any room the bot happens to be a member
                // of would accept the trigger phrase.
                // ------------------------------------------------

                let source_room_id = room.room_id().to_string();

                if !trigger_rooms
                    .iter()
                    .any(|allowed| *allowed == source_room_id)
                {
                    return;
                }

                // ------------------------------------------------
                // READ MESSAGE BODY
                // ------------------------------------------------

                let body = match &event {
                    SyncRoomMessageEvent::Original(ev) => {
                        ev.content.body()
                    }

                    SyncRoomMessageEvent::Redacted(_) => {
                        return;
                    }
                };

                // ------------------------------------------------
                // CHECK TRIGGER
                // ------------------------------------------------

                if body.trim() != welcome_trigger {
                    return;
                }

                let sender = event.sender();

                let sender_key =
                    sender.to_string();

                // ------------------------------------------------
                // DEBOUNCE
                // ------------------------------------------------

                {
                    let mut map =
                        debounce.lock().unwrap();

                    if let Some(last) =
                        map.get(&sender_key)
                    {
                        if last.elapsed()
                            < Duration::from_secs(
                                debounce_seconds,
                            )
                        {
                            println!(
                                "Ignoring duplicate {} from {}",
                                welcome_trigger,
                                sender_key
                            );

                            return;
                        }
                    }

                    map.insert(
                        sender_key.clone(),
                        Instant::now(),
                    );
                }

                // ------------------------------------------------
                // LOG TRIGGER
                // ------------------------------------------------

                println!();
                println!("==========================================");
                println!("WELCOME TRIGGERED");
                println!("==========================================");
                println!("Sender: {}", sender);
                println!(
                    "Trigger room: {}",
                    room.room_id()
                );
                println!(
                    "Target rooms: {}",
                    target_rooms.len()
                );
                println!();

                // ------------------------------------------------
                // PROCESS EVERY TARGET ROOM
                // ------------------------------------------------

                for target_room_string in &target_rooms {
                    let target_room_id =
                        match RoomId::parse(
                            target_room_string,
                        ) {
                            Ok(id) => id,

                            Err(error) => {
                                eprintln!(
                                    "Invalid target room {}: {}",
                                    target_room_string,
                                    error
                                );

                                continue;
                            }
                        };

                    println!(
                        "Target room: {}",
                        target_room_id
                    );

                    // --------------------------------------------
                    // GET OR JOIN TARGET ROOM
                    // --------------------------------------------

                    let target_room =
                        match client.get_room(
                            &target_room_id,
                        ) {
                            Some(room) => {
                                println!(
                                    "Target room already known \
                                     to this device."
                                );

                                room
                            }

                            None => {
                                println!(
                                    "Target room is not in the \
                                     local cache yet."
                                );

                                println!(
                                    "Attempting to join target \
                                     room by ID..."
                                );

                                match client
                                    .join_room_by_id(
                                        &target_room_id,
                                    )
                                    .await
                                {
                                    Ok(room) => {
                                        println!(
                                            "Target room obtained \
                                             successfully."
                                        );

                                        // Force one client-level
                                        // sync after joining.
                                        //
                                        // Note: see the caveat
                                        // below about sync_once.
                                        if let Err(error) =
                                            client
                                                .sync_once(
                                                    SyncSettings::default(),
                                                )
                                                .await
                                        {
                                            eprintln!(
                                                "Post-join sync failed: {:#}",
                                                error
                                            );
                                        }

                                        room
                                    }

                                    Err(error) => {
                                        eprintln!(
                                            "Could not obtain target \
                                             room {}: {:#}",
                                            target_room_id,
                                            error
                                        );

                                        continue;
                                    }
                                }
                            }
                        };

                    // --------------------------------------------
                    // DIAGNOSTIC EXPORT
                    // --------------------------------------------

                    if let Err(error) =
                        diagnose_target_rooms(
                            &client,
                            std::slice::from_ref(
                                target_room_string,
                            ),
                            &diagnostic_key_file,
                        )
                        .await
                    {
                        eprintln!(
                            "Diagnostic failed for {}: {:#}",
                            target_room_id,
                            error
                        );
                    }

                    // --------------------------------------------
                    // INVITE SENDER
                    // --------------------------------------------

                    println!();
                    println!(
                        "Inviting {} to {}...",
                        sender,
                        target_room.room_id()
                    );

                    match target_room
                        .invite_user_by_id(&sender)
                        .await
                    {
                        Ok(_) => {
                            println!();
                            println!(
                                "Invite successfully sent."
                            );

                            println!(
                                "MSC4268 history sharing was \
                                 handled by the Matrix SDK."
                            );
                        }

                        Err(error) => {
                            eprintln!();
                            eprintln!(
                                "Failed to invite {} to {}:",
                                sender,
                                target_room.room_id()
                            );

                            eprintln!("{:#}", error);
                        }
                    }

                    println!();
                }

                println!();
                println!("WELCOME COMPLETE");
                println!("==========================================");
                println!();
            }
        },
    );
}
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs, io,
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use futures_util::StreamExt;
use ksni::blocking::TrayMethods;
use stuk_platform::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutActivation, GlobalShortcutRegistration,
    NativeMessagingHost, PlatformEvent, SingleInstanceActivation, SingleInstancePolicy,
    TrayActivation, TrayIcon, TrayMenuItem,
};

type EventQueue = Arc<Mutex<Vec<PlatformEvent>>>;

pub struct LinuxDesktopServiceState {
    events: EventQueue,
    tray: Option<TrayRuntime>,
    shortcuts: BTreeMap<String, ShortcutRuntime>,
    single_instance: Option<SingleInstanceGuard>,
}

impl std::fmt::Debug for LinuxDesktopServiceState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LinuxDesktopServiceState")
            .field(
                "queued_events",
                &self.events.lock().map(|events| events.len()).ok(),
            )
            .field("shortcuts", &self.shortcuts.keys().collect::<Vec<_>>())
            .field("single_instance", &self.single_instance.is_some())
            .finish()
    }
}

impl LinuxDesktopServiceState {
    fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
            tray: None,
            shortcuts: BTreeMap::new(),
            single_instance: None,
        }
    }

    pub fn take_events(&self) -> Vec<PlatformEvent> {
        self.events
            .lock()
            .map(|mut events| events.drain(..).collect())
            .unwrap_or_default()
    }
}

pub fn apply_linux_desktop_services(
    tray_icon: Option<&TrayIcon>,
    autostart: &[AutostartEntry],
    global_shortcuts: &[GlobalShortcutRegistration],
    deep_links: &[DeepLinkRegistration],
    native_messaging_hosts: &[NativeMessagingHost],
    single_instance_id: Option<&str>,
    single_instance_policy: Option<SingleInstancePolicy>,
) -> Result<LinuxDesktopServiceState, String> {
    let mut state = LinuxDesktopServiceState::new();
    if let Some(policy) = single_instance_policy
        && policy != SingleInstancePolicy::AllowMultiple
    {
        state.single_instance = Some(SingleInstanceGuard::acquire(
            single_instance_id,
            policy,
            Arc::clone(&state.events),
        )?);
    }
    for entry in autostart {
        write_autostart_entry(entry).map_err(|error| error.to_string())?;
    }
    if let Some(icon) = tray_icon {
        state.tray = Some(spawn_tray_icon(icon, Arc::clone(&state.events))?);
    }
    for registration in global_shortcuts {
        state.shortcuts.insert(
            registration.id.clone(),
            spawn_global_shortcut(registration, Arc::clone(&state.events)),
        );
    }
    for registration in deep_links {
        register_deep_links(registration).map_err(|error| error.to_string())?;
    }
    for host in native_messaging_hosts {
        register_native_messaging_host(host).map_err(|error| error.to_string())?;
    }
    Ok(state)
}

fn spawn_tray_icon(icon: &TrayIcon, events: EventQueue) -> Result<TrayRuntime, String> {
    let tray = LinuxTray {
        icon: icon.clone(),
        events,
    };
    tray.assume_sni_available(true)
        .spawn()
        .map(|handle| TrayRuntime { handle })
        .map_err(|error| error.to_string())
}

fn spawn_global_shortcut(
    registration: &GlobalShortcutRegistration,
    events: EventQueue,
) -> ShortcutRuntime {
    let registration = registration.clone();
    let thread = thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        let _ = runtime.block_on(run_portal_shortcut(registration, events));
    });
    ShortcutRuntime { thread }
}

struct TrayRuntime {
    handle: ksni::blocking::Handle<LinuxTray>,
}

impl Drop for TrayRuntime {
    fn drop(&mut self) {
        self.handle.shutdown().wait();
    }
}

struct ShortcutRuntime {
    thread: JoinHandle<()>,
}

impl Drop for ShortcutRuntime {
    fn drop(&mut self) {
        let _ = self.thread.thread().id();
    }
}

#[derive(Clone)]
struct LinuxTray {
    icon: TrayIcon,
    events: EventQueue,
}

impl LinuxTray {
    fn push(&self, event: PlatformEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }
}

impl ksni::Tray for LinuxTray {
    fn id(&self) -> String {
        sanitize_desktop_id(&self.icon.id)
    }

    fn title(&self) -> String {
        self.icon.title.clone()
    }

    fn icon_name(&self) -> String {
        self.icon
            .icon_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| "application-x-executable".to_string())
    }

    fn icon_theme_path(&self) -> String {
        self.icon
            .icon_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(|path| path.display().to_string())
            .unwrap_or_default()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.icon.title.clone(),
            description: self.icon.tooltip.clone().unwrap_or_default(),
            ..ksni::ToolTip::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.push(PlatformEvent::Tray(TrayActivation::new(
            self.icon.id.clone(),
        )));
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        self.icon.menu.iter().map(tray_menu_item).collect()
    }
}

fn tray_menu_item(item: &TrayMenuItem) -> ksni::MenuItem<LinuxTray> {
    if item.separator {
        return ksni::MenuItem::Separator;
    }
    let item_id = item.id.clone();
    let action = item.action.clone();
    let label = item.label.clone();
    let enabled = item.enabled;
    ksni::menu::StandardItem {
        label,
        enabled,
        activate: Box::new(move |tray: &mut LinuxTray| {
            tray.push(PlatformEvent::Tray(TrayActivation::item(
                tray.icon.id.clone(),
                item_id.clone(),
                action.clone(),
            )));
        }),
        ..ksni::menu::StandardItem::default()
    }
    .into()
}

async fn run_portal_shortcut(
    registration: GlobalShortcutRegistration,
    events: EventQueue,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use ashpd::desktop::{
        CreateSessionOptions,
        global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut},
    };

    ensure_portal_app_registration(&registration).await?;

    let Some(trigger) = portal_trigger_for_shortcut(&registration) else {
        return Err("global shortcut is not supported by the Linux portal backend".into());
    };
    let portal = GlobalShortcuts::new().await?;
    let session = portal
        .create_session(CreateSessionOptions::default())
        .await?;
    let mut activations = portal.receive_activated().await?;
    let description = registration
        .description
        .as_deref()
        .unwrap_or(&registration.action);
    let shortcut = NewShortcut::new(registration.id.as_str(), description)
        .preferred_trigger(Some(trigger.as_str()));
    let request = portal
        .bind_shortcuts(&session, &[shortcut], None, BindShortcutsOptions::default())
        .await?;
    let response = request.response()?;
    if !response
        .shortcuts()
        .iter()
        .any(|shortcut| shortcut.id() == registration.id)
    {
        return Err("the portal did not bind the requested shortcut".into());
    }

    while let Some(event) = activations.next().await {
        if event.shortcut_id() != registration.id {
            continue;
        }
        let mut activation =
            GlobalShortcutActivation::new(registration.id.clone(), registration.action.clone());
        if let Some(token) = activation_token_from_options(event.options()) {
            activation = activation.activation_token(token);
        }
        if let Ok(mut events) = events.lock() {
            events.push(PlatformEvent::GlobalShortcut(activation));
        }
    }
    Ok(())
}

fn portal_trigger_for_shortcut(registration: &GlobalShortcutRegistration) -> Option<String> {
    let shortcut = &registration.shortcut;
    let mut parts = Vec::new();
    if shortcut.modifiers.ctrl {
        parts.push("CTRL".to_string());
    }
    if shortcut.modifiers.alt {
        parts.push("ALT".to_string());
    }
    if shortcut.modifiers.shift {
        parts.push("SHIFT".to_string());
    }
    if shortcut.modifiers.meta {
        parts.push("LOGO".to_string());
    }
    let mut key = shortcut.key.trim().to_string();
    if key.is_empty() {
        return None;
    }
    if key.len() == 1 && key.is_ascii() {
        key.make_ascii_lowercase();
    }
    parts.push(key);
    Some(parts.join("+"))
}

fn activation_token_from_options(
    options: &std::collections::HashMap<String, ashpd::zvariant::OwnedValue>,
) -> Option<String> {
    let value = options.get("activation_token")?.try_clone().ok()?;
    String::try_from(value)
        .ok()
        .filter(|token| !token.trim().is_empty())
}

async fn ensure_portal_app_registration(
    registration: &GlobalShortcutRegistration,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(app_id) = registration.app_id.as_deref() else {
        return Ok(());
    };
    if let Some(command) = registration.desktop_command.as_deref() {
        let app_name = registration
            .app_name
            .as_deref()
            .or(registration.description.as_deref())
            .unwrap_or(app_id);
        write_file(
            data_home()?
                .join("applications")
                .join(format!("{}.desktop", sanitize_desktop_id(app_id))),
            &desktop_entry(app_id, app_name, command),
        )?;
    }
    let app_id = ashpd::AppID::try_from(app_id)?;
    match ashpd::register_host_app(app_id).await {
        Ok(()) => Ok(()),
        Err(error) if portal_app_already_registered(&error) => Ok(()),
        Err(error) => Err(Box::new(error)),
    }
}

fn portal_app_already_registered(error: &ashpd::Error) -> bool {
    let message = error.to_string();
    message.contains("already associated") || message.contains("already registered")
}

struct SingleInstanceGuard {
    socket_path: PathBuf,
    thread: JoinHandle<()>,
}

impl SingleInstanceGuard {
    fn acquire(
        instance_id: Option<&str>,
        policy: SingleInstancePolicy,
        events: EventQueue,
    ) -> Result<Self, String> {
        let socket_path =
            single_instance_socket_path(instance_id).map_err(|error| error.to_string())?;
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        match UnixListener::bind(&socket_path) {
            Ok(listener) => Ok(spawn_single_instance_listener(
                socket_path,
                listener,
                policy,
                events,
            )),
            Err(_) if send_single_instance_activation(&socket_path, policy).is_ok() => {
                Err("another instance is already running".to_string())
            }
            Err(_) => {
                let _ = fs::remove_file(&socket_path);
                let listener =
                    UnixListener::bind(&socket_path).map_err(|error| error.to_string())?;
                Ok(spawn_single_instance_listener(
                    socket_path,
                    listener,
                    policy,
                    events,
                ))
            }
        }
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        let _ = self.thread.thread().id();
        let _ = fs::remove_file(&self.socket_path);
    }
}

fn spawn_single_instance_listener(
    socket_path: PathBuf,
    listener: UnixListener,
    policy: SingleInstancePolicy,
    events: EventQueue,
) -> SingleInstanceGuard {
    let thread = thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if let Some(activation) = read_single_instance_activation(policy, stream)
                && let Ok(mut events) = events.lock()
            {
                events.push(PlatformEvent::SingleInstance(activation));
            }
        }
    });
    SingleInstanceGuard {
        socket_path,
        thread,
    }
}

fn read_single_instance_activation(
    policy: SingleInstancePolicy,
    mut stream: UnixStream,
) -> Option<SingleInstanceActivation> {
    let mut body = String::new();
    stream.read_to_string(&mut body).ok()?;
    let value: serde_json::Value = serde_json::from_str(&body).ok()?;
    let arguments = value
        .get("arguments")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut activation = SingleInstanceActivation::new(policy, arguments);
    if let Some(cwd) = value.get("cwd").and_then(|value| value.as_str()) {
        activation = activation.working_directory(cwd);
    }
    if let Some(token) = value
        .get("activationToken")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        activation = activation.activation_token(token);
    }
    Some(activation)
}

fn send_single_instance_activation(
    socket_path: &PathBuf,
    policy: SingleInstancePolicy,
) -> io::Result<()> {
    let mut stream = UnixStream::connect(socket_path)?;
    let cwd = env::current_dir()
        .ok()
        .map(|path| path.display().to_string());
    let body = serde_json::json!({
        "policy": format!("{policy:?}"),
        "arguments": env::args().collect::<Vec<_>>(),
        "cwd": cwd,
        "activationToken": startup_activation_token(),
    });
    stream.write_all(body.to_string().as_bytes())
}

fn startup_activation_token() -> Option<String> {
    env::var("XDG_ACTIVATION_TOKEN")
        .or_else(|_| env::var("DESKTOP_STARTUP_ID"))
        .ok()
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

fn single_instance_socket_path(instance_id: Option<&str>) -> io::Result<PathBuf> {
    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir());
    let id = instance_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(sanitize_desktop_id)
        .map(Ok)
        .unwrap_or_else(|| {
            env::current_exe().map(|exe| {
                exe.file_stem()
                    .and_then(|name| name.to_str())
                    .map(sanitize_desktop_id)
                    .unwrap_or_else(|| "app".to_string())
            })
        })?;
    Ok(runtime.join("fenestra").join(format!("{id}.sock")))
}

pub fn start_desktop_event_forwarder(
    services: &LinuxDesktopServiceState,
    running: Arc<AtomicBool>,
    mut emit: impl FnMut(PlatformEvent) + Send + 'static,
) -> JoinHandle<()> {
    let events = Arc::clone(&services.events);
    thread::spawn(move || {
        while running.load(Ordering::Relaxed) {
            let drained = events
                .lock()
                .map(|mut events| events.drain(..).collect::<Vec<_>>())
                .unwrap_or_default();
            for event in drained {
                emit(event);
            }
            thread::sleep(Duration::from_millis(8));
        }
    })
}

fn write_autostart_entry(entry: &AutostartEntry) -> io::Result<()> {
    let path = config_home()?
        .join("autostart")
        .join(format!("{}.desktop", sanitize_desktop_id(&entry.id)));
    if !entry.enabled {
        match fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    write_file(path, &desktop_entry(&entry.id, &entry.name, &entry.command))
}

fn register_deep_links(registration: &DeepLinkRegistration) -> io::Result<()> {
    let desktop_id = format!("{}.desktop", sanitize_desktop_id(&registration.id));
    let path = config_home()?.join("mimeapps.list");
    let mut content = fs::read_to_string(&path).unwrap_or_default();
    for scheme in &registration.schemes {
        let scheme = sanitize_scheme(scheme);
        if !scheme.is_empty() {
            content = set_mime_default(&content, &scheme, &desktop_id);
        }
    }
    write_file(path, &content)
}

fn register_native_messaging_host(host: &NativeMessagingHost) -> io::Result<()> {
    let name = sanitize_native_host_name(&host.id);
    let chrome_manifest = native_messaging_manifest(host, &name, "allowed_origins");
    for browser in ["google-chrome", "chromium", "BraveSoftware/Brave-Browser"] {
        write_file(
            config_home()?
                .join(browser)
                .join("NativeMessagingHosts")
                .join(format!("{name}.json")),
            &chrome_manifest,
        )?;
    }
    let firefox_manifest = native_messaging_manifest(host, &name, "allowed_extensions");
    write_file(
        home_dir()?
            .join(".mozilla/native-messaging-hosts")
            .join(format!("{name}.json")),
        &firefox_manifest,
    )
}

fn write_file(path: PathBuf, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}

fn desktop_entry(id: &str, name: &str, command: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName={}\nGenericName={}\nComment={}\nExec={}\nIcon={}\nTerminal=false\nNoDisplay=true\nStartupNotify=false\nCategories=Utility;\n",
        desktop_value(name),
        desktop_value(name),
        desktop_value(name),
        desktop_value(command),
        desktop_value(id)
    )
}

fn set_mime_default(content: &str, scheme: &str, desktop_id: &str) -> String {
    let key = format!("x-scheme-handler/{scheme}");
    let value = format!("{key}={desktop_id}");
    let mut lines = content.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let Some(section_start) = lines
        .iter()
        .position(|line| line.trim() == "[Default Applications]")
    else {
        if !lines.is_empty() && lines.last().is_some_and(|line| !line.is_empty()) {
            lines.push(String::new());
        }
        lines.push("[Default Applications]".to_string());
        lines.push(value);
        return finish_lines(lines);
    };
    let section_end = lines
        .iter()
        .enumerate()
        .skip(section_start + 1)
        .find_map(|(index, line)| line.trim().starts_with('[').then_some(index))
        .unwrap_or(lines.len());
    if let Some(index) = lines[section_start + 1..section_end]
        .iter()
        .position(|line| {
            line.split_once('=')
                .is_some_and(|(line_key, _)| line_key == key)
        })
    {
        lines[section_start + 1 + index] = value;
    } else {
        lines.insert(section_end, value);
    }
    finish_lines(lines)
}

fn native_messaging_manifest(host: &NativeMessagingHost, name: &str, allowed_key: &str) -> String {
    let allowed = host
        .allowed_origins
        .iter()
        .map(|origin| format!("\"{}\"", json_value(origin)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    format!(
        "{{\n  \"name\": \"{}\",\n  \"description\": \"{}\",\n  \"path\": \"{}\",\n  \"type\": \"stdio\",\n  \"{}\": [{}]\n}}\n",
        json_value(name),
        json_value(&host.name),
        json_value(&host.executable.display().to_string()),
        allowed_key,
        allowed.join(", ")
    )
}

fn finish_lines(lines: Vec<String>) -> String {
    let mut output = lines.join("\n");
    output.push('\n');
    output
}

fn desktop_value(value: &str) -> String {
    value.replace(['\n', '\r'], " ")
}

fn json_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn sanitize_desktop_id(value: &str) -> String {
    sanitize_with(value, |ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_')
    })
}

fn sanitize_scheme(value: &str) -> String {
    sanitize_with(&value.to_ascii_lowercase(), |ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '+' | '.' | '-')
    })
}

fn sanitize_native_host_name(value: &str) -> String {
    sanitize_with(&value.to_ascii_lowercase(), |ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.')
    })
}

fn sanitize_with(value: &str, valid: impl Fn(char) -> bool) -> String {
    let sanitized = value
        .chars()
        .map(|ch| if valid(ch) { ch } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if sanitized.is_empty() {
        "app".to_string()
    } else {
        sanitized
    }
}

fn config_home() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path));
    }
    Ok(home_dir()?.join(".config"))
}

fn data_home() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(path));
    }
    Ok(home_dir()?.join(".local/share"))
}

fn home_dir() -> io::Result<PathBuf> {
    env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "HOME is required for Linux desktop integration",
        )
    })
}

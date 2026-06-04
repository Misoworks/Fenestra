use std::{
    io::{self, BufRead, BufReader, Write},
    net::Shutdown,
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::Child,
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use layershellev::calloop::channel::Sender;

use crate::{
    HOST_CONTROL_PREFIX,
    osr_protocol::{OsrMessage, read_message},
};

pub(super) enum LayerHostEvent {
    Connected(UnixStream),
    Message(OsrMessage),
    Visible(bool),
    Disconnected,
}

pub(super) fn spawn_layer_bridge_proxy(child: &mut Child, sender: Sender<LayerHostEvent>) {
    if let Some(stdout) = child.stdout.take() {
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            let mut output = io::stdout();
            for line in reader.lines().map_while(std::result::Result::ok) {
                if writeln!(output, "{line}").is_err() {
                    break;
                }
                let _ = output.flush();
            }
        });
    }

    if let Some(mut stdin) = child.stdin.take() {
        thread::spawn(move || {
            let input = io::stdin();
            for line in input.lock().lines().map_while(std::result::Result::ok) {
                if let Some(visible) = parse_visibility_control(&line) {
                    if sender.send(LayerHostEvent::Visible(visible)).is_err() {
                        break;
                    }
                    continue;
                }
                if line.starts_with(HOST_CONTROL_PREFIX) {
                    continue;
                }
                if writeln!(stdin, "{line}").is_err() {
                    break;
                }
                let _ = stdin.flush();
            }
        });
    }
}

pub(super) fn open_socket_reader(sender: Sender<LayerHostEvent>) -> Option<PathBuf> {
    let socket_path = osr_socket_path();
    let _ = std::fs::remove_file(&socket_path);
    let listener = match UnixListener::bind(&socket_path) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("failed to bind Fenestra layer OSR socket: {error}");
            return None;
        }
    };
    start_socket_reader(listener, sender);
    Some(socket_path)
}

fn start_socket_reader(listener: UnixListener, sender: Sender<LayerHostEvent>) {
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        if let Ok(writer) = stream.try_clone() {
            let _ = sender.send(LayerHostEvent::Connected(writer));
        }
        loop {
            match read_message(&mut stream) {
                Ok(Some(message)) => {
                    if sender.send(LayerHostEvent::Message(message)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
                    ) =>
                {
                    break;
                }
                Err(error) => {
                    eprintln!("Fenestra layer OSR socket read failed: {error}");
                    break;
                }
            }
        }
        let _ = stream.shutdown(Shutdown::Both);
        let _ = sender.send(LayerHostEvent::Disconnected);
    });
}

fn osr_socket_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "fenestra-layer-osr-{}-{nanos}.sock",
        std::process::id()
    ))
}

fn parse_visibility_control(line: &str) -> Option<bool> {
    let (command, value) = crate::parse_host_control(line)?;
    match command {
        "visible" => match value {
            "1" | "true" | "yes" | "show" | "visible" => Some(true),
            "0" | "false" | "no" | "hide" | "hidden" => Some(false),
            _ => None,
        },
        "show" | "focus" => Some(true),
        "hide" => Some(false),
        _ => None,
    }
}

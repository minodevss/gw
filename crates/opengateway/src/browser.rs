use std::process::{Command, Stdio};

const SSH_VARIABLES: [&str; 3] = ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"];

pub(crate) fn is_remote() -> bool {
    SSH_VARIABLES
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

pub(crate) async fn open(url: &str) -> bool {
    if is_remote() {
        return false;
    }
    let Some(opener) = opener() else {
        return false;
    };
    let url = url.to_owned();
    tokio::task::spawn_blocking(move || {
        Command::new(opener)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
    .await
    .unwrap_or(false)
}

fn opener() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("open")
    } else if cfg!(target_os = "linux") {
        Some("xdg-open")
    } else {
        None
    }
}

use std::{
    path::Path,
    process::{Command, Stdio},
};

use crate::source_install::SourceApp;

pub fn entry(app: &SourceApp, wrapper: &Path, desktop_icon: Option<&str>) -> String {
    let icon = desktop_icon.unwrap_or(&app.id);
    let mime_types = mime_type_line(&app.mime_types);
    format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec={} %U\nIcon={}\n{}Terminal=false\nCategories=Utility;\nStartupNotify=true\nStartupWMClass={}\n",
        desktop_value(&app.name),
        desktop_exec(wrapper),
        desktop_value(icon),
        mime_types,
        desktop_value(&app.id)
    )
}

pub fn refresh_database(applications_dir: &Path) {
    if !command_exists("update-desktop-database") {
        return;
    }
    let _ = Command::new("update-desktop-database")
        .arg(applications_dir)
        .stdin(Stdio::null())
        .status();
}

fn mime_type_line(mime_types: &[String]) -> String {
    if mime_types.is_empty() {
        return String::new();
    }
    let values = mime_types
        .iter()
        .map(|mime_type| mime_type.trim())
        .filter(|mime_type| !mime_type.is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        String::new()
    } else {
        format!("MimeType={};\n", values.join(";"))
    }
}

fn desktop_value(value: &str) -> String {
    value.replace(['\n', '\r'], " ")
}

fn desktop_exec(path: &Path) -> String {
    path.display().to_string().replace(' ', "\\ ")
}

fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|path| {
            let candidate = path.join(name);
            candidate.is_file()
        })
    })
}

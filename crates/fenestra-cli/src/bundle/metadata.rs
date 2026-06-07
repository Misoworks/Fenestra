use super::{BundleFormat, config::BundleApp};

pub(super) fn bundle_toml(app: &BundleApp, format: BundleFormat, executable: &str) -> String {
    let web_dist = app
        .web
        .as_ref()
        .filter(|web| web.has_local_assets)
        .map(|web| web.dist.display().to_string())
        .unwrap_or_default();
    format!(
        "format = \"{}\"\napp_id = \"{}\"\nname = \"{}\"\nversion = \"{}\"\nbinary = \"{}\"\nweb_dist = \"{}\"\n",
        format.as_str(),
        quote(&app.id),
        quote(&app.name),
        quote(&app.version),
        quote(executable),
        quote(&web_dist)
    )
}

pub(super) fn web_toml(app: &BundleApp) -> Option<String> {
    let web = app.web.as_ref()?;
    let allowed_origins = web
        .allowed_origins
        .iter()
        .map(|origin| format!("\"{}\"", quote(origin)))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "root = \"{}\"\ndist = \"{}\"\nentry = \"{}\"\nbuild = \"{}\"\nurl = \"{}\"\ndev_url = \"{}\"\nallowed_origins = [{}]\n",
        quote(&web.root.display().to_string()),
        quote(&web.dist.display().to_string()),
        quote(&web.entry.display().to_string()),
        quote(web.build_command.as_deref().unwrap_or_default()),
        quote(web.url.as_deref().unwrap_or_default()),
        quote(web.dev_url.as_deref().unwrap_or_default()),
        allowed_origins
    ))
}

pub(super) fn desktop_entry(app: &BundleApp, executable: &str, icon: Option<&str>) -> String {
    let icon = icon
        .map(|icon| format!("Icon={}\n", desktop_value(icon)))
        .unwrap_or_default();
    let mime_types = mime_type_line(&app.mime_types);
    format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec={}\n{}{}Terminal=false\nCategories=Utility;Development;\nStartupNotify=true\n",
        desktop_value(&app.name),
        desktop_value(executable),
        icon,
        mime_types
    )
}

pub(super) fn app_run(executable: &str) -> String {
    format!(
        "#!/bin/sh\nHERE=\"$(dirname \"$(readlink -f \"$0\")\")\"\nexec \"$HERE/usr/bin/{executable}\" \"$@\"\n"
    )
}

pub(super) fn info_plist(app: &BundleApp, executable: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{}</string>
<key>CFBundleName</key><string>{}</string>
<key>CFBundleDisplayName</key><string>{}</string>
<key>CFBundleExecutable</key><string>{}</string>
<key>CFBundleVersion</key><string>{}</string>
<key>CFBundleShortVersionString</key><string>{}</string>
<key>LSMinimumSystemVersion</key><string>12.0</string>
</dict></plist>
"#,
        xml(&app.id),
        xml(&app.name),
        xml(&app.name),
        xml(executable),
        xml(&app.version),
        xml(&app.version)
    )
}

pub(super) fn windows_manifest(app: &BundleApp) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity version="{}.0" processorArchitecture="*" name="{}" type="win32"/>
  <description>{}</description>
  <dependency><dependentAssembly><assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/></dependentAssembly></dependency>
</assembly>
"#,
        xml(&app.version),
        xml(&app.id),
        xml(&app.name)
    )
}

pub(super) fn flatpak_manifest(app: &BundleApp, executable: &str) -> String {
    format!(
        "{{\"app-id\":\"{}\",\"runtime\":\"org.freedesktop.Platform\",\"runtime-version\":\"24.08\",\"sdk\":\"org.freedesktop.Sdk\",\"command\":\"{}\",\"modules\":[]}}\n",
        json(&app.id),
        json(executable)
    )
}

pub(super) fn deb_control(app: &BundleApp, installed_size_kb: u64) -> String {
    format!(
        "Package: {}\nVersion: {}\nSection: utils\nPriority: optional\nArchitecture: amd64\nMaintainer: Fenestra <noreply@example.invalid>\nInstalled-Size: {}\nDepends: libc6\nDescription: {}\n",
        debian_name(&app.id),
        app.version,
        installed_size_kb.max(1),
        app.name
    )
}

pub(super) fn rpm_spec(app: &BundleApp, executable: &str) -> String {
    format!(
        r#"Name: {name}
Version: {version}
Release: 1%{{?dist}}
Summary: {summary}
License: unknown
BuildArch: x86_64

%description
{summary}

%files
/usr/bin/{executable}
/usr/share/applications/{id}.desktop
/usr/share/fenestra/{id}
"#,
        name = rpm_name(&app.id),
        version = app.version,
        summary = app.name,
        executable = executable,
        id = app.id
    )
}

pub(super) fn wix_source(app: &BundleApp, executable_source: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Wix xmlns="http://wixtoolset.org/schemas/v4/wxs">
  <Package Name="{}" Manufacturer="Fenestra" Version="{}" UpgradeCode="PUT-GENERATED-UPGRADE-CODE-HERE">
    <MediaTemplate EmbedCab="yes"/>
    <StandardDirectory Id="ProgramFilesFolder">
      <Directory Id="INSTALLFOLDER" Name="{}">
        <File Source="{}"/>
      </Directory>
    </StandardDirectory>
  </Package>
</Wix>
"#,
        xml(&app.name),
        xml(&app.version),
        xml(&app.name),
        xml(executable_source)
    )
}

pub(super) fn nsis_script(
    app: &BundleApp,
    staged_app_dir: &str,
    executable: &str,
    output: &str,
) -> String {
    format!(
        "Name \"{}\"\nOutFile \"{}\"\nInstallDir \"$PROGRAMFILES64\\{}\"\nSection\nSetOutPath \"$INSTDIR\"\nFile /r \"{}/*\"\nCreateShortcut \"$DESKTOP\\{}.lnk\" \"$INSTDIR\\{}\"\nSectionEnd\n",
        app.name, output, app.name, staged_app_dir, app.name, executable
    )
}

pub(super) fn shell_script(lines: &[&str]) -> String {
    format!("#!/bin/sh\nset -e\n{}\n", lines.join("\n"))
}

pub(super) fn sanitize_path(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(super) fn json(value: &str) -> String {
    quote(value)
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn desktop_value(value: &str) -> String {
    value.replace(['\n', '\r'], " ")
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

fn debian_name(value: &str) -> String {
    value.to_ascii_lowercase().replace('_', "-")
}

fn rpm_name(value: &str) -> String {
    value.replace('.', "-").replace('_', "-")
}

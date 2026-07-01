use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug, PartialEq)]
pub struct BridgeCommand {
    pub name: String,
    pub params: serde_json::Value,
    pub origin: Option<String>,
}

#[derive(Clone, Debug)]
pub struct BridgeResponse {
    pub result: serde_json::Value,
}

impl BridgeResponse {
    pub fn json(result: serde_json::Value) -> Self {
        Self { result }
    }
}

#[derive(Clone, Debug)]
pub struct BridgeError {
    pub message: String,
}

impl BridgeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub type BridgeResult = std::result::Result<BridgeResponse, BridgeError>;
type BridgeHandler = Arc<dyn Fn(BridgeCommand) -> BridgeResult + Send + Sync>;

#[derive(Clone, Default)]
pub struct BridgeHandlers {
    handlers: BTreeMap<String, BridgeHandler>,
}

impl std::fmt::Debug for BridgeHandlers {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BridgeHandlers")
            .field("commands", &self.commands())
            .finish()
    }
}

impl BridgeHandlers {
    pub fn register<F>(&mut self, command_name: impl Into<String>, handler: F)
    where
        F: Fn(BridgeCommand) -> BridgeResult + Send + Sync + 'static,
    {
        self.handlers.insert(command_name.into(), Arc::new(handler));
    }

    pub fn contains(&self, command_name: &str) -> bool {
        self.handlers.contains_key(command_name)
    }

    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    pub fn commands(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    pub fn dispatch(&self, command: BridgeCommand) -> BridgeResult {
        let Some(handler) = self.handlers.get(&command.name) else {
            return Err(BridgeError::new(format!(
                "Bridge command `{}` is not registered",
                command.name
            )));
        };
        handler(command)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BridgeCommandDescriptor {
    pub name: String,
    pub description: Option<String>,
    pub params_schema: Option<serde_json::Value>,
    pub permissions: Vec<String>,
    pub allowed_origins: Vec<String>,
    pub targets: Vec<String>,
}

impl BridgeCommandDescriptor {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            params_schema: None,
            permissions: Vec::new(),
            allowed_origins: Vec::new(),
            targets: Vec::new(),
        }
    }

    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.targets.push(target.into());
        self
    }

    pub fn permission(mut self, permission: impl Into<String>) -> Self {
        self.permissions.push(permission.into());
        self
    }

    pub fn allowed_origin(mut self, origin: impl Into<String>) -> Self {
        self.allowed_origins.push(origin.into());
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct BridgeRegistry {
    commands: Vec<BridgeCommandDescriptor>,
}

impl BridgeRegistry {
    pub fn register(&mut self, command_name: impl Into<String>) {
        self.register_descriptor(BridgeCommandDescriptor::new(command_name));
    }

    pub fn register_descriptor(&mut self, command: BridgeCommandDescriptor) {
        if !self
            .commands
            .iter()
            .any(|existing| existing.name == command.name)
        {
            self.commands.push(command);
        }
    }

    pub fn descriptor(&self, command_name: &str) -> Option<&BridgeCommandDescriptor> {
        self.commands
            .iter()
            .find(|command| command.name == command_name)
    }

    pub fn commands(&self) -> Vec<String> {
        self.commands
            .iter()
            .map(|command| command.name.clone())
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct BridgeRuntime {
    handlers: BridgeHandlers,
    registry: BridgeRegistry,
    security: WebViewSecurity,
}

impl BridgeRuntime {
    pub fn new(
        handlers: BridgeHandlers,
        registry: BridgeRegistry,
        security: WebViewSecurity,
    ) -> Self {
        Self {
            handlers,
            registry,
            security,
        }
    }

    pub fn dispatch(&self, command: BridgeCommand) -> BridgeResult {
        let descriptor = self.registry.descriptor(&command.name);
        validate_permissions(&self.security, &command, descriptor)?;
        validate_targets(&command, descriptor)?;
        validate_origin(&self.security, &command, descriptor)?;
        self.handlers.dispatch(command)
    }

    pub fn security(&self) -> &WebViewSecurity {
        &self.security
    }
}

#[derive(Clone, Debug, Default)]
pub struct WebViewSecurity {
    pub remote_content: bool,
    pub allowed_origins: Vec<String>,
    pub allowed_bridge_permissions: Vec<String>,
}

impl WebViewSecurity {
    pub fn allow_origin(mut self, origin: impl Into<String>) -> Self {
        self.remote_content = true;
        let origin = origin.into();
        if !self
            .allowed_origins
            .iter()
            .any(|allowed| allowed == &origin)
        {
            self.allowed_origins.push(origin);
        }
        self
    }

    pub fn allow_bridge_permission(mut self, permission: impl Into<String>) -> Self {
        let permission = permission.into();
        if !self
            .allowed_bridge_permissions
            .iter()
            .any(|allowed| allowed == &permission)
        {
            self.allowed_bridge_permissions.push(permission);
        }
        self
    }

    pub fn remote_content(mut self, enabled: bool) -> Self {
        self.remote_content = enabled;
        self
    }
}

fn validate_permissions(
    security: &WebViewSecurity,
    command: &BridgeCommand,
    descriptor: Option<&BridgeCommandDescriptor>,
) -> std::result::Result<(), BridgeError> {
    let Some(descriptor) = descriptor else {
        return Ok(());
    };
    for permission in &descriptor.permissions {
        if !security
            .allowed_bridge_permissions
            .iter()
            .any(|allowed| allowed == permission || allowed == "*")
        {
            return Err(BridgeError::new(format!(
                "Bridge command `{}` requires permission `{permission}`",
                command.name
            )));
        }
    }
    Ok(())
}

fn validate_targets(
    command: &BridgeCommand,
    descriptor: Option<&BridgeCommandDescriptor>,
) -> std::result::Result<(), BridgeError> {
    let Some(descriptor) = descriptor else {
        return Ok(());
    };
    if descriptor.targets.is_empty()
        || descriptor
            .targets
            .iter()
            .any(|target| current_bridge_targets().contains(&target.as_str()))
    {
        return Ok(());
    }
    Err(BridgeError::new(format!(
        "Bridge command `{}` is unavailable on this target",
        command.name
    )))
}

fn validate_origin(
    security: &WebViewSecurity,
    command: &BridgeCommand,
    descriptor: Option<&BridgeCommandDescriptor>,
) -> std::result::Result<(), BridgeError> {
    let Some(origin) = command.origin.as_deref() else {
        return Ok(());
    };
    if origin == "null" || origin.starts_with("file://") || origin.starts_with("devtools://") {
        return Ok(());
    }
    let command_origins = descriptor
        .map(|descriptor| descriptor.allowed_origins.as_slice())
        .unwrap_or(&[]);
    if origin_matches(origin, command_origins)
        || (security.remote_content && origin_matches(origin, &security.allowed_origins))
    {
        return Ok(());
    }
    Err(BridgeError::new(format!(
        "Bridge command `{}` is not allowed from origin `{origin}`",
        command.name
    )))
}

fn origin_matches(origin: &str, allowed: &[String]) -> bool {
    allowed.iter().any(|candidate| {
        candidate == origin
            || candidate == "*"
            || (candidate.ends_with("/*") && origin.starts_with(candidate.trim_end_matches('*')))
    })
}

/// List of bridge targets this build reports to the page. The CEF and
/// WebView2 backends set the host side of this; the bridge validation logic
/// uses it to filter commands that have an explicit target list.
pub fn current_bridge_targets() -> &'static [&'static str] {
    #[cfg(target_os = "linux")]
    return &["desktop", "linux"];
    #[cfg(target_os = "windows")]
    return &["desktop", "windows"];
    #[cfg(target_os = "macos")]
    return &["desktop", "macos"];
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    return &["mobile"];
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &str) -> BridgeCommand {
        BridgeCommand {
            name: name.to_string(),
            params: serde_json::json!({ "value": 1 }),
            origin: Some("file:///tmp/index.html".to_string()),
        }
    }

    #[test]
    fn dispatches_registered_command() {
        let mut handlers = BridgeHandlers::default();
        handlers.register("notes.list", |command| {
            Ok(BridgeResponse::json(
                serde_json::json!({ "name": command.name }),
            ))
        });
        let mut registry = BridgeRegistry::default();
        registry.register("notes.list");
        let runtime = BridgeRuntime::new(handlers, registry, WebViewSecurity::default());

        let response = runtime.dispatch(command("notes.list")).unwrap();
        assert_eq!(response.result["name"], "notes.list");
    }

    #[test]
    fn rejects_command_without_handler() {
        let handlers = BridgeHandlers::default();
        let mut registry = BridgeRegistry::default();
        registry.register("notes.list");
        let runtime = BridgeRuntime::new(handlers, registry, WebViewSecurity::default());

        let error = runtime.dispatch(command("notes.list")).unwrap_err();
        assert!(error.message.contains("not registered"));
    }

    #[test]
    fn enforces_bridge_permissions() {
        let mut handlers = BridgeHandlers::default();
        handlers.register("vault.unlock", |_| {
            Ok(BridgeResponse::json(serde_json::json!(true)))
        });
        let mut registry = BridgeRegistry::default();
        registry
            .register_descriptor(BridgeCommandDescriptor::new("vault.unlock").permission("vault"));
        let runtime = BridgeRuntime::new(handlers, registry, WebViewSecurity::default());

        let error = runtime.dispatch(command("vault.unlock")).unwrap_err();
        assert!(error.message.contains("requires permission"));
    }

    #[test]
    fn accepts_allowed_remote_origin() {
        let mut handlers = BridgeHandlers::default();
        handlers.register("notes.list", |_| {
            Ok(BridgeResponse::json(serde_json::json!([])))
        });
        let mut registry = BridgeRegistry::default();
        registry.register("notes.list");
        let runtime = BridgeRuntime::new(
            handlers,
            registry,
            WebViewSecurity {
                remote_content: true,
                allowed_origins: vec!["https://app.example".to_string()],
                allowed_bridge_permissions: Vec::new(),
            },
        );
        let mut request = command("notes.list");
        request.origin = Some("https://app.example".to_string());

        assert!(runtime.dispatch(request).is_ok());
    }

    #[test]
    fn rejects_unallowed_remote_origin() {
        let mut handlers = BridgeHandlers::default();
        handlers.register("notes.list", |_| {
            Ok(BridgeResponse::json(serde_json::json!([])))
        });
        let mut registry = BridgeRegistry::default();
        registry.register("notes.list");
        let runtime = BridgeRuntime::new(handlers, registry, WebViewSecurity::default());
        let mut request = command("notes.list");
        request.origin = Some("https://app.example".to_string());

        let error = runtime.dispatch(request).unwrap_err();
        assert!(error.message.contains("not allowed"));
    }

    #[test]
    fn rejects_wrong_target() {
        let mut handlers = BridgeHandlers::default();
        handlers.register("mobile.only", |_| {
            Ok(BridgeResponse::json(serde_json::json!(true)))
        });
        let mut registry = BridgeRegistry::default();
        registry.register_descriptor(BridgeCommandDescriptor::new("mobile.only").target("mobile"));
        let runtime = BridgeRuntime::new(handlers, registry, WebViewSecurity::default());

        let error = runtime.dispatch(command("mobile.only")).unwrap_err();
        assert!(error.message.contains("unavailable"));
    }
}

use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug)]
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

    pub(crate) fn dispatch(&self, command: BridgeCommand) -> BridgeResult {
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
pub(crate) struct BridgeRuntime {
    handlers: BridgeHandlers,
    registry: BridgeRegistry,
    security: WebViewSecurity,
}

impl BridgeRuntime {
    pub(crate) fn new(
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

    pub(crate) fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    pub(crate) fn dispatch(&self, command: BridgeCommand) -> BridgeResult {
        let descriptor = self.registry.descriptor(&command.name);
        validate_permissions(&self.security, &command, descriptor)?;
        validate_targets(&command, descriptor)?;
        validate_origin(&self.security, &command, descriptor)?;
        self.handlers.dispatch(command)
    }
}

#[derive(Clone, Debug)]
pub struct WebViewSecurity {
    pub remote_content: bool,
    pub allowed_origins: Vec<String>,
    pub allowed_bridge_permissions: Vec<String>,
}

impl Default for WebViewSecurity {
    fn default() -> Self {
        Self {
            remote_content: false,
            allowed_origins: Vec::new(),
            allowed_bridge_permissions: Vec::new(),
        }
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

fn current_bridge_targets() -> &'static [&'static str] {
    #[cfg(target_os = "linux")]
    return &["desktop", "linux"];
    #[cfg(target_os = "windows")]
    return &["desktop", "windows"];
    #[cfg(target_os = "macos")]
    return &["desktop", "macos"];
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    return &["mobile"];
}

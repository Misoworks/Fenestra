use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use serde_json::{Value, json};

use crate::{BridgeCommand, BridgeError, BridgeResponse, BridgeResult};

pub(crate) const BEGIN_COMMAND: &str = "fenestra.activity.begin";
pub(crate) const END_COMMAND: &str = "fenestra.activity.end";
pub(crate) const LIST_COMMAND: &str = "fenestra.activity.list";

const INTERNAL_COMMANDS: [&str; 3] = [BEGIN_COMMAND, END_COMMAND, LIST_COMMAND];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityOptions {
    pub name: String,
    pub prevents_hibernation: bool,
}

impl ActivityOptions {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            prevents_hibernation: true,
        }
    }

    pub fn prevents_hibernation(mut self, prevents_hibernation: bool) -> Self {
        self.prevents_hibernation = prevents_hibernation;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityRecord {
    pub id: String,
    pub name: String,
    pub prevents_hibernation: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ActivityRegistry {
    inner: Arc<Mutex<ActivityState>>,
}

pub struct CefActivityLease {
    registry: ActivityRegistry,
    emitter: Option<crate::BridgeEventEmitter>,
    record: Option<ActivityRecord>,
}

#[derive(Debug, Default)]
struct ActivityState {
    next_id: u64,
    records: BTreeMap<String, ActivityRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ActivityHostUpdate {
    Begin(ActivityRecord),
    End(ActivityRecord),
}

impl ActivityRegistry {
    pub(crate) fn begin(&self, options: ActivityOptions) -> ActivityRecord {
        let mut state = self.inner.lock().expect("activity registry poisoned");
        state.next_id += 1;
        let record = ActivityRecord {
            id: format!("activity-{}", state.next_id),
            name: normalize_name(&options.name),
            prevents_hibernation: options.prevents_hibernation,
        };
        state.records.insert(record.id.clone(), record.clone());
        record
    }

    pub(crate) fn end(&self, id: &str) -> Option<ActivityRecord> {
        self.inner
            .lock()
            .expect("activity registry poisoned")
            .records
            .remove(id)
    }

    pub(crate) fn list(&self) -> Vec<ActivityRecord> {
        self.inner
            .lock()
            .expect("activity registry poisoned")
            .records
            .values()
            .cloned()
            .collect()
    }

    pub(crate) fn dispatch_bridge_command(
        &self,
        command: &BridgeCommand,
    ) -> Option<(BridgeResult, Option<ActivityHostUpdate>)> {
        match command.name.as_str() {
            BEGIN_COMMAND => Some(self.dispatch_begin(command)),
            END_COMMAND => Some(self.dispatch_end(command)),
            LIST_COMMAND => Some((Ok(BridgeResponse::json(self.snapshot_json())), None)),
            _ => None,
        }
    }

    pub(crate) fn lease(
        &self,
        options: ActivityOptions,
        emitter: Option<crate::BridgeEventEmitter>,
    ) -> CefActivityLease {
        let record = self.begin(options);
        if let Some(emitter) = &emitter {
            let _ = emitter.emit_activity_update(&ActivityHostUpdate::Begin(record.clone()));
        }
        CefActivityLease {
            registry: self.clone(),
            emitter,
            record: Some(record),
        }
    }

    fn dispatch_begin(
        &self,
        command: &BridgeCommand,
    ) -> (BridgeResult, Option<ActivityHostUpdate>) {
        let options = ActivityOptions {
            name: command
                .params
                .get("name")
                .and_then(Value::as_str)
                .map(normalize_name)
                .unwrap_or_else(|| "activity".to_string()),
            prevents_hibernation: bool_param(&command.params, "preventsHibernation")
                .or_else(|| bool_param(&command.params, "prevents_hibernation"))
                .unwrap_or(true),
        };
        let record = self.begin(options);
        (
            Ok(BridgeResponse::json(record_json(&record))),
            Some(ActivityHostUpdate::Begin(record)),
        )
    }

    fn dispatch_end(&self, command: &BridgeCommand) -> (BridgeResult, Option<ActivityHostUpdate>) {
        let Some(id) = command.params.get("id").and_then(Value::as_str) else {
            return (
                Err(BridgeError::new("activity end requires an `id` string")),
                None,
            );
        };
        let Some(record) = self.end(id) else {
            return (
                Ok(BridgeResponse::json(json!({ "id": id, "ended": false }))),
                None,
            );
        };
        (
            Ok(BridgeResponse::json(json!({
                "id": record.id.clone(),
                "ended": true
            }))),
            Some(ActivityHostUpdate::End(record)),
        )
    }

    fn snapshot_json(&self) -> Value {
        let activities = self.list();
        json!({
            "activities": activities.iter().map(record_json).collect::<Vec<_>>(),
            "hibernationBlockers": activities
                .iter()
                .filter(|activity| activity.prevents_hibernation)
                .count(),
        })
    }
}

impl CefActivityLease {
    pub fn id(&self) -> Option<&str> {
        self.record.as_ref().map(|record| record.id.as_str())
    }

    pub fn record(&self) -> Option<&ActivityRecord> {
        self.record.as_ref()
    }

    pub fn end(mut self) {
        self.end_inner();
    }

    fn end_inner(&mut self) {
        let Some(record) = self.record.take() else {
            return;
        };
        if let Some(record) = self.registry.end(&record.id)
            && let Some(emitter) = &self.emitter
        {
            let _ = emitter.emit_activity_update(&ActivityHostUpdate::End(record));
        }
    }
}

impl Drop for CefActivityLease {
    fn drop(&mut self) {
        self.end_inner();
    }
}

pub(crate) fn bridge_commands_with_internal(commands: Vec<String>) -> Vec<String> {
    let mut commands = commands;
    for command in INTERNAL_COMMANDS {
        if !commands.iter().any(|existing| existing == command) {
            commands.push(command.to_string());
        }
    }
    commands
}

pub(crate) fn host_update_json(update: &ActivityHostUpdate) -> Value {
    match update {
        ActivityHostUpdate::Begin(record) => json!({
            "id": record.id,
            "name": record.name,
            "preventsHibernation": record.prevents_hibernation,
            "active": true,
        }),
        ActivityHostUpdate::End(record) => json!({
            "id": record.id,
            "name": record.name,
            "preventsHibernation": record.prevents_hibernation,
            "active": false,
        }),
    }
}

fn bool_param(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
}

fn normalize_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "activity".to_string();
    }
    trimmed.chars().take(128).collect()
}

fn record_json(record: &ActivityRecord) -> Value {
    json!({
        "id": record.id,
        "name": record.name,
        "preventsHibernation": record.prevents_hibernation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &str, params: Value) -> BridgeCommand {
        BridgeCommand {
            name: name.to_string(),
            params,
            origin: None,
        }
    }

    #[test]
    fn bridge_commands_include_activity_commands() {
        let commands = bridge_commands_with_internal(vec!["notes.list".to_string()]);
        assert!(commands.iter().any(|command| command == "notes.list"));
        assert!(commands.iter().any(|command| command == BEGIN_COMMAND));
        assert!(commands.iter().any(|command| command == END_COMMAND));
        assert!(commands.iter().any(|command| command == LIST_COMMAND));
    }

    #[test]
    fn begin_defaults_to_hibernation_blocker() {
        let registry = ActivityRegistry::default();
        let (response, update) = registry
            .dispatch_bridge_command(&command(BEGIN_COMMAND, json!({ "name": "backup" })))
            .expect("activity command");

        let response = response.expect("activity response").result;
        assert_eq!(response["name"], "backup");
        assert_eq!(response["preventsHibernation"], true);
        assert!(matches!(update, Some(ActivityHostUpdate::Begin(_))));
        assert_eq!(registry.list().len(), 1);
    }

    #[test]
    fn end_removes_activity() {
        let registry = ActivityRegistry::default();
        let record = registry.begin(ActivityOptions::new("indexing"));
        let (response, update) = registry
            .dispatch_bridge_command(&command(END_COMMAND, json!({ "id": record.id })))
            .expect("activity command");

        assert_eq!(response.expect("activity response").result["ended"], true);
        assert!(matches!(update, Some(ActivityHostUpdate::End(_))));
        assert!(registry.list().is_empty());
    }
}

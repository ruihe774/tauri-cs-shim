//! Event types: `EventTarget`, `EventEnvelope`, the `Emitter` and `Listener`
//! traits, and `EventId`. The broadcast bus and listener registry live on
//! `AppInner` in `manager.rs`.

use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where an event is addressed. Mirrors upstream's `EventTarget`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum EventTarget {
    /// Broadcast to every listener.
    Any,
    /// Match listeners whose label equals this string. Note the upstream quirk
    /// (tauri#11561): plain `Any` listeners do *not* match `AnyLabel` events.
    AnyLabel { label: String },
    /// App-level event source / target.
    App,
    Window { label: String },
    Webview { label: String },
    WebviewWindow { label: String },
}

impl EventTarget {
    /// Convenience extractor for the SSE payload `source` field.
    pub fn label(&self) -> Option<&str> {
        match self {
            EventTarget::AnyLabel { label }
            | EventTarget::Window { label }
            | EventTarget::Webview { label }
            | EventTarget::WebviewWindow { label } => Some(label),
            EventTarget::Any | EventTarget::App => None,
        }
    }
}

impl From<&str> for EventTarget {
    fn from(label: &str) -> Self {
        EventTarget::AnyLabel {
            label: label.to_string(),
        }
    }
}

impl From<String> for EventTarget {
    fn from(label: String) -> Self {
        EventTarget::AnyLabel { label }
    }
}

/// Identifier handed back from `Listener::listen` / `once`.
pub type EventId = u32;

pub(crate) static NEXT_EVENT_ID: AtomicU32 = AtomicU32::new(1);

pub(crate) fn next_event_id() -> EventId {
    NEXT_EVENT_ID.fetch_add(1, Ordering::Relaxed)
}

/// One unit broadcast on the bus. Carries the addressing metadata so SSE
/// clients and backend listeners can both filter.
#[derive(Debug, Clone, Serialize)]
pub struct EventEnvelope {
    pub id: EventId,
    pub event: String,
    pub payload: Value,
    pub target: EventTarget,
    pub source: EventTarget,
}

/// Event delivered to a backend listener registered via `Listener::listen`.
#[derive(Debug, Clone)]
pub struct Event {
    pub id: EventId,
    pub event: String,
    pub payload: Value,
    pub source: EventTarget,
}

impl Event {
    /// Deserialize the payload as `T`. Borrows from `Value` clone, so any
    /// `serde::de::DeserializeOwned` works.
    pub fn payload_as<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.payload.clone())
    }
}

/// Filter routing logic shared by SSE clients and backend dispatch.
///
/// Routing follows real Tauri's semantics:
/// - `Any`-target events broadcast to every labelled listener.
/// - Labelled events only reach listeners with the same label.
/// - The upstream quirk (tauri#11561): a plain `Any` *listener* does NOT
///   match `AnyLabel` events. Only `Any`-target events match an `Any`
///   listener. Preserved verbatim so user code that worked against real
///   Tauri also works here.
pub fn event_matches_listener(target: &EventTarget, listener: &EventTarget) -> bool {
    match listener {
        EventTarget::Any => matches!(target, EventTarget::Any),
        EventTarget::App => matches!(target, EventTarget::Any | EventTarget::App),
        EventTarget::AnyLabel { label }
        | EventTarget::Window { label }
        | EventTarget::Webview { label }
        | EventTarget::WebviewWindow { label } => match target {
            EventTarget::Any => true,
            EventTarget::App => false,
            other => other.label() == Some(label.as_str()),
        },
    }
}

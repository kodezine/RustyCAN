//! Adapter registry (issue #111): a pluggable alternative to the hardcoded
//! [`AdapterKind`] enum and the per-variant `match` in [`open_adapter`].
//!
//! **Step 1 (this module)** defines the registration surface and wraps today's
//! built-ins as factories that *delegate* to the existing `open_adapter` /
//! `probe_adapter_kind`, so behavior is unchanged. A follow-up migrates the
//! Connect UI and persisted config to drive off this registry, at which point
//! the closed enum can be retired.

// Registry is not yet wired into the UI/config; that lands in a follow-up (#111).
#![allow(dead_code)]

use super::{open_adapter, probe_adapter_kind, AdapterError, AdapterKind, CanAdapter};

/// Connection parameters gathered from the Connect form. Deliberately generic:
/// each factory reads only the fields it needs, so new adapters can require
/// different inputs without changing a shared enum.
#[derive(Clone, Default)]
pub struct AdapterParams {
    pub port: String,
    pub baud: u32,
    pub listen_only: bool,
    /// Optional USB serial used to pin a specific device (KCAN, Apex).
    pub serial: Option<String>,
}

/// Three-state adapter availability (Issue B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterAvailability {
    /// Detected and not claimed by any other process.
    Available,
    /// Detected but already claimed by another running RustyCAN instance.
    InUse,
    /// Not detected (device absent or interface down).
    Absent,
}

/// A self-describing CAN adapter backend the UI and config can discover at
/// runtime instead of hardcoding it into an enum + `match`.
pub trait CanAdapterFactory: Send + Sync {
    /// Stable identifier used in persisted config (replaces the closed enum tag).
    fn id(&self) -> &'static str;

    /// Human-readable name for the Connect-form radio option.
    fn display_name(&self) -> &'static str;

    /// Open a live adapter with the collected connection parameters.
    fn open(&self, params: &AdapterParams) -> Result<Box<dyn CanAdapter>, AdapterError>;

    /// Three-state presence check for the connect-form adapter list.
    ///
    /// Default delegates to [`Self::probe`] → `Available` / `Absent`.
    /// Override to add `InUse` detection (SocketCAN does this via lockfile).
    fn availability(&self, params: &AdapterParams) -> AdapterAvailability {
        if self.probe(params) {
            AdapterAvailability::Available
        } else {
            AdapterAvailability::Absent
        }
    }

    /// Legacy boolean probe; used by the default `availability()` impl.
    fn probe(&self, params: &AdapterParams) -> bool;
}

/// The set of adapter backends available to the app. Built-ins register here at
/// startup; out-of-tree adapters can register their own factory too.
#[derive(Default)]
pub struct AdapterRegistry {
    factories: Vec<Box<dyn CanAdapterFactory>>,
}

impl AdapterRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, factory: Box<dyn CanAdapterFactory>) {
        self.factories.push(factory);
    }

    pub fn factories(&self) -> &[Box<dyn CanAdapterFactory>] {
        &self.factories
    }

    pub fn by_id(&self, id: &str) -> Option<&dyn CanAdapterFactory> {
        self.factories
            .iter()
            .map(|f| f.as_ref())
            .find(|f| f.id() == id)
    }

    /// The built-in adapters, registered as factories that delegate to the
    /// existing enum-based [`open_adapter`] — no behavior change.
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        r.register(Box::new(BuiltinFactory::SUMMIT));
        r.register(Box::new(BuiltinFactory::KCAN));
        r.register(Box::new(BuiltinFactory::SOCKETCAN));
        r.register(Box::new(BuiltinFactory::APEX));
        r
    }
}

/// Bridges an existing [`AdapterKind`] variant to the new factory surface so the
/// built-ins keep their exact current behavior during the migration.
struct BuiltinFactory {
    id: &'static str,
    name: &'static str,
    make_kind: fn(&AdapterParams) -> AdapterKind,
}

impl BuiltinFactory {
    const SUMMIT: Self = Self {
        id: "summit",
        name: "Summit",
        make_kind: |p| AdapterKind::Summit {
            channel: p.port.clone(),
        },
    };
    const KCAN: Self = Self {
        id: "kcan",
        name: "KCAN Dongle",
        make_kind: |p| AdapterKind::KCan {
            serial: p.serial.clone(),
        },
    };
    const SOCKETCAN: Self = Self {
        id: "socketcan",
        name: "SocketCAN",
        make_kind: |p| AdapterKind::SocketCan {
            iface: p.port.clone(),
        },
    };
    const APEX: Self = Self {
        id: "apex",
        name: "Apex",
        make_kind: |p| AdapterKind::Apex {
            serial: p.serial.clone(),
        },
    };
}

impl CanAdapterFactory for BuiltinFactory {
    fn id(&self) -> &'static str {
        self.id
    }

    fn display_name(&self) -> &'static str {
        self.name
    }

    fn open(&self, p: &AdapterParams) -> Result<Box<dyn CanAdapter>, AdapterError> {
        open_adapter(&(self.make_kind)(p), p.baud, p.listen_only)
    }

    fn availability(&self, p: &AdapterParams) -> AdapterAvailability {
        // SocketCAN: check lockfile to detect InUse before falling back to probe.
        #[cfg(target_os = "linux")]
        if self.id == "socketcan" {
            let iface = &p.port;
            let lock_path = std::path::PathBuf::from(format!("/tmp/rustycan-{iface}.pid"));
            if lock_path.exists() {
                if let Ok(contents) = std::fs::read_to_string(&lock_path) {
                    if let Ok(pid) = contents.trim().parse::<u32>() {
                        if std::path::Path::new(&format!("/proc/{pid}")).exists() {
                            return AdapterAvailability::InUse;
                        }
                    }
                }
            }
        }
        if self.probe(p) {
            AdapterAvailability::Available
        } else {
            AdapterAvailability::Absent
        }
    }

    fn probe(&self, p: &AdapterParams) -> bool {
        probe_adapter_kind(&(self.make_kind)(p))
    }
}

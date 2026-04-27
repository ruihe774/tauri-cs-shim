//! No-op stub for the upstream `tauri-build` crate.
//!
//! User `build.rs` scripts that call `tauri_build::build()` (or the more
//! involved `Attributes`-based forms) compile against this shim without
//! touching real Tauri's icon/capability/codegen machinery. Under the
//! debug shim there is no bundle to assemble.

/// Mirrors `tauri_build::build`. Returns immediately.
pub fn build() {}

/// Mirrors `tauri_build::try_build`. Always succeeds.
pub fn try_build(_attributes: Attributes) -> Result<(), Error> {
    Ok(())
}

/// Stub mirroring upstream's builder. All setters return `self` and do
/// nothing; the resulting value is consumed by `try_build`.
#[derive(Debug, Default, Clone)]
pub struct Attributes {
    _private: (),
}

impl Attributes {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn codegen(self, _codegen: CodegenContext) -> Self {
        self
    }

    pub fn plugin(self, _name: &str, _attributes: InlinedPlugin) -> Self {
        self
    }

    pub fn app_manifest(self, _manifest: AppManifest) -> Self {
        self
    }
}

#[derive(Debug, Default, Clone)]
pub struct CodegenContext {
    _private: (),
}

impl CodegenContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn dev(self) -> Self {
        self
    }
}

#[derive(Debug, Default, Clone)]
pub struct InlinedPlugin {
    _private: (),
}

impl InlinedPlugin {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Default, Clone)]
pub struct AppManifest {
    _private: (),
}

impl AppManifest {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Error returned by `try_build`. The shim never produces one; the type
/// exists for API compatibility.
#[derive(Debug)]
pub struct Error;

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("tauri-build (shim) error")
    }
}

impl std::error::Error for Error {}

//! Nitro extension host adapter over the existing WasmSandbox.

use crate::kernel_handle::KernelHandle;
use crate::sandbox::{SandboxConfig, WasmSandbox};
use openfang_types::capability::Capability;
use openfang_types::nitro::{NitroExtensionManifest, NitroWasmAbi};
use std::sync::{Arc, OnceLock};

fn shared_sandbox() -> &'static WasmSandbox {
    static SANDBOX: OnceLock<WasmSandbox> = OnceLock::new();
    SANDBOX.get_or_init(|| {
        WasmSandbox::new()
            .unwrap_or_else(|e| panic!("Failed to initialize Nitro extension sandbox: {e}"))
    })
}

/// Host that runs Nitro extension manifests in the shared WASM runtime.
#[derive(Default)]
pub struct NitroExtensionHost;

impl NitroExtensionHost {
    pub fn new() -> Self {
        Self
    }

    /// Execute an extension manifest entrypoint with capability payload.
    pub async fn execute_manifest(
        &self,
        manifest: &NitroExtensionManifest,
        payload: serde_json::Value,
        kernel: Option<Arc<dyn KernelHandle>>,
        agent_id: &str,
    ) -> Result<serde_json::Value, String> {
        if manifest.abi != NitroWasmAbi::AllocExecute {
            return Err(format!(
                "Unsupported Nitro WASM ABI '{}'.",
                serde_json::to_string(&manifest.abi).unwrap_or_default()
            ));
        }

        let wasm_path = std::path::Path::new(&manifest.entry);
        if !wasm_path.exists() {
            return Err(format!(
                "Extension entry WASM not found at '{}'.",
                wasm_path.display()
            ));
        }

        let wasm_bytes = std::fs::read(wasm_path).map_err(|e| {
            format!(
                "Failed to read extension WASM '{}' for execution: {e}",
                wasm_path.display()
            )
        })?;

        let mut caps: Vec<Capability> = manifest
            .capabilities
            .iter()
            .map(|c| Capability::ToolInvoke(c.name.clone()))
            .collect();

        for required in &manifest.required_host_apis {
            match required.as_str() {
                "time_now" => {}
                "fs_read" | "fs_list" => caps.push(Capability::FileRead("*".to_string())),
                "fs_write" => caps.push(Capability::FileWrite("*".to_string())),
                "net_fetch" => caps.push(Capability::NetConnect("*".to_string())),
                "shell_exec" => caps.push(Capability::ShellExec("*".to_string())),
                "kv_get" => caps.push(Capability::MemoryRead("*".to_string())),
                "kv_set" => caps.push(Capability::MemoryWrite("*".to_string())),
                "agent_send" => caps.push(Capability::AgentMessage("*".to_string())),
                "agent_spawn" => caps.push(Capability::AgentSpawn),
                _ => {}
            }
        }

        let cfg = SandboxConfig {
            fuel_limit: 2_000_000,
            max_memory_bytes: 32 * 1024 * 1024,
            capabilities: caps,
            timeout_secs: Some(30),
        };

        let result = shared_sandbox()
            .execute(&wasm_bytes, payload, cfg, kernel, agent_id)
            .await
            .map_err(|e| format!("Nitro extension execution failed: {e}"))?;

        Ok(result.output)
    }
}

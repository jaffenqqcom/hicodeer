use anyhow::{Result, bail};
use async_trait::async_trait;
use collections::HashMap;
use dap::{StartDebuggingRequestArguments, adapters::DebugTaskDefinition};
use gpui::AsyncApp;
use std::ffi::OsStr;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use task::{DebugScenario, ZedDebugConfig};

use crate::*;

/// Drives `lldb-vscode`, the DAP front end that ships with LLVM. Nothing is
/// downloaded and no executable is rewritten, so it behaves like the adapters
/// that ship with the platform rather than the CodeLLDB one. On this platform
/// the session runs over TCP; see `get_binary` for why stdio cannot be used.
#[derive(Default)]
pub(crate) struct LldbVscodeDebugAdapter;

impl LldbVscodeDebugAdapter {
    const ADAPTER_NAME: &'static str = "LLDB";
    const EXECUTABLE_NAME: &'static str = "lldb-vscode";
    const CONFIG_PATH_KEY: &'static str = "lldb_vscode_path";
    const CONFIG_ARGS_KEY: &'static str = "lldb_vscode_args";
}

#[async_trait(?Send)]
impl DebugAdapter for LldbVscodeDebugAdapter {
    fn name(&self) -> DebugAdapterName {
        DebugAdapterName(Self::ADAPTER_NAME.into())
    }

    async fn config_from_zed_format(&self, zed_scenario: ZedDebugConfig) -> Result<DebugScenario> {
        let mut obj = serde_json::Map::default();

        match &zed_scenario.request {
            dap::DebugRequest::Attach(attach) => {
                obj.insert("request".into(), "attach".into());
                obj.insert("pid".into(), attach.process_id.into());
            }

            dap::DebugRequest::Launch(launch) => {
                obj.insert("request".into(), "launch".into());
                obj.insert("program".into(), launch.program.clone().into());

                if !launch.args.is_empty() {
                    obj.insert("args".into(), launch.args.clone().into());
                }

                if !launch.env.is_empty() {
                    obj.insert("env".into(), launch.env_json());
                }

                if let Some(cwd) = launch.cwd.as_ref() {
                    obj.insert("cwd".into(), cwd.to_string_lossy().into_owned().into());
                }

                if let Some(stop_on_entry) = zed_scenario.stop_on_entry {
                    obj.insert("stopOnEntry".into(), stop_on_entry.into());
                }
            }
        }

        Ok(DebugScenario {
            adapter: zed_scenario.adapter,
            label: zed_scenario.label,
            build: None,
            config: serde_json::Value::Object(obj),
            tcp_connection: None,
        })
    }

    fn dap_schema(&self) -> serde_json::Value {
        json!({
            "oneOf": [
                {
                    "allOf": [
                        {
                            "type": "object",
                            "required": ["request"],
                            "properties": {
                                "request": {
                                    "type": "string",
                                    "enum": ["launch"],
                                    "description": "Request to launch a new process"
                                }
                            }
                        },
                        {
                            "type": "object",
                            "properties": {
                                "program": {
                                    "type": "string",
                                    "description": "The executable to debug."
                                },
                                "args": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Command line arguments passed to the program.",
                                    "default": []
                                },
                                "cwd": {
                                    "type": "string",
                                    "description": "Working directory for the debugged program."
                                },
                                "env": {
                                    "type": "object",
                                    "description": "Environment variables for the debugged program."
                                },
                                "stopOnEntry": {
                                    "type": "boolean",
                                    "description": "Stop at the first instruction of the program.",
                                    "default": false
                                },
                                "lldb_vscode_path": {
                                    "type": "string",
                                    "description": "Alternative path to the lldb-vscode executable."
                                },
                                "lldb_vscode_args": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Extra arguments given to lldb-vscode itself, not to the program.",
                                    "default": []
                                }
                            },
                            "required": ["program"]
                        }
                    ]
                },
                {
                    "allOf": [
                        {
                            "type": "object",
                            "required": ["request"],
                            "properties": {
                                "request": {
                                    "type": "string",
                                    "enum": ["attach"],
                                    "description": "Request to attach to an existing process"
                                }
                            }
                        },
                        {
                            "type": "object",
                            "properties": {
                                "pid": {
                                    "type": "number",
                                    "description": "The process ID to attach to."
                                },
                                "program": {
                                    "type": "string",
                                    "description": "The executable that the process is running."
                                },
                                "lldb_vscode_path": {
                                    "type": "string",
                                    "description": "Alternative path to the lldb-vscode executable."
                                }
                            },
                            "required": ["pid"]
                        }
                    ]
                }
            ]
        })
    }

    async fn get_binary(
        &self,
        delegate: &Arc<dyn DapDelegate>,
        config: &DebugTaskDefinition,
        user_installed_path: Option<PathBuf>,
        user_args: Option<Vec<String>>,
        user_env: Option<HashMap<String, String>>,
        _: &mut AsyncApp,
    ) -> Result<DebugAdapterBinary> {
        let configured_path = config
            .config
            .get(Self::CONFIG_PATH_KEY)
            .and_then(|value| value.as_str())
            .map(|path| path.to_string());

        let executable = if let Some(path) = configured_path {
            path
        } else {
            let user_setting_path = user_installed_path
                .filter(|path| path.exists())
                .and_then(|path| path.to_str().map(|path| path.to_string()));

            let resolved = delegate
                .which(OsStr::new(Self::EXECUTABLE_NAME))
                .await
                .and_then(|path| path.to_str().map(|path| path.to_string()));

            match (user_setting_path, resolved) {
                (Some(path), _) => path,
                (None, Some(path)) => path,
                (None, None) => bail!(
                    "Could not find {} in PATH; set {} in the debug configuration",
                    Self::EXECUTABLE_NAME,
                    Self::CONFIG_PATH_KEY
                ),
            }
        };

        let mut arguments = config
            .config
            .get(Self::CONFIG_ARGS_KEY)
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(|value| value.to_string()))
                    .collect::<Vec<_>>()
            })
            .or(user_args)
            .unwrap_or_default();

        let mut configuration = config.config.clone();
        if let Some(configuration) = configuration.as_object_mut() {
            configuration.remove(Self::CONFIG_PATH_KEY);
            configuration.remove(Self::CONFIG_ARGS_KEY);
            configuration
                .entry("cwd")
                .or_insert_with(|| delegate.worktree_root_path().to_string_lossy().into());
        }

        let mut environment = delegate.shell_env().await;
        environment.extend(user_env.unwrap_or_default());

        let config_env: HashMap<String, String> = config
            .config
            .get("env")
            .and_then(|value| value.as_object())
            .map(|object| {
                object
                    .iter()
                    .filter_map(|(key, value)| {
                        value.as_str().map(|value| (key.clone(), value.to_string()))
                    })
                    .collect::<HashMap<String, String>>()
            })
            .unwrap_or_default();

        environment.extend(config_env);

        // lldb-vscode aborts when it drives a session over stdio on this
        // platform (SIGABRT right after the `process` event) while its TCP mode
        // completes normally, so the adapter listens on a loopback port and the
        // client connects to it.
        let host = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let port = dap::transport::TcpTransport::unused_port(host).await?;
        arguments.push("-p".to_string());
        arguments.push(port.to_string());

        Ok(DebugAdapterBinary {
            command: Some(executable),
            arguments,
            envs: environment,
            cwd: Some(delegate.worktree_root_path().to_path_buf()),
            connection: Some(adapters::TcpArguments {
                host,
                port,
                timeout: None,
            }),
            request_args: StartDebuggingRequestArguments {
                request: self.request_kind(&configuration).await?,
                configuration,
            },
        })
    }
}

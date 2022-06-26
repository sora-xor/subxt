// Copyright 2019-2022 Parity Technologies (UK) Ltd.
// This file is part of subxt.
//
// subxt is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// subxt is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with subxt.  If not, see <http://www.gnu.org/licenses/>.

use std::{
    env,
    fs,
    net::TcpListener,
    ops::{
        Deref,
        DerefMut,
    },
    path::Path,
    process::Command,
    thread,
    time,
};
use subxt::rpc::{
    self,
    ClientT,
};

static SUBSTRATE_BIN_ENV_VAR: &str = "SUBSTRATE_NODE_PATH";

#[tokio::main]
async fn main() {
    run().await;
}

async fn run() {
    // Select substrate binary to run based on env var.
    let substrate_bin =
        env::var(SUBSTRATE_BIN_ENV_VAR).unwrap_or_else(|_| "substrate".to_owned());

    // Run binary.
    let port = next_open_port().expect("Cannot spawn substrate: no available ports");
    let cmd = Command::new(&substrate_bin)
        .arg("--dev")
        .arg("--tmp")
        .arg(format!("--ws-port={}", port))
        .spawn();
    let mut cmd = match cmd {
        Ok(cmd) => KillOnDrop(cmd),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => {
            panic!("A substrate binary should be installed on your path for testing purposes. \
            See https://github.com/paritytech/subxt/tree/master#integration-testing")
        }
        Err(e) => {
            panic!("Cannot spawn substrate command '{}': {}", substrate_bin, e)
        }
    };

    // Download metadata from binary; retry until successful, or a limit is hit.
    let metadata_bytes: sp_core::Bytes = {
        const MAX_RETRIES: usize = 6;
        let mut retries = 0;

        loop {
            if retries >= MAX_RETRIES {
                panic!("Cannot connect to substrate node after {} retries", retries);
            }

            // It might take a while for substrate node that spin up the RPC server.
            // Thus, the connection might get rejected a few times.
            let res = match rpc::ws_client(&format!("ws://localhost:{}", port)).await {
                Ok(c) => c.request("state_getMetadata", None).await,
                Err(e) => Err(e),
            };

            match res {
                Ok(res) => {
                    let _ = cmd.kill();
                    break res
                }
                _ => {
                    thread::sleep(time::Duration::from_secs(1 << retries));
                    retries += 1;
                }
            };
        }
    };

    // Save metadata to a file:
    let out_dir = env::var_os("OUT_DIR").unwrap();
    let metadata_path = Path::new(&out_dir).join("metadata.scale");
    fs::write(&metadata_path, &metadata_bytes.0).expect("Couldn't write metadata output");

    // Write out our expression to generate the runtime API to a file. Ideally, we'd just write this code
    // in lib.rs, but we must pass a string literal (and not `concat!(..)`) as an arg to `runtime_metadata_path`,
    // and so we need to spit it out here and include it verbatim instead.
    let runtime_api_contents = format!(
        r#"
        #[subxt::subxt(
            runtime_metadata_path = "{}",
            derive_for_all_types = "Eq, PartialEq"
        )]
        pub mod node_runtime {{
            #[subxt(substitute_type = "sp_arithmetic::per_things::Perbill")]
            use ::sp_runtime::Perbill;
        }}
    "#,
        metadata_path
            .to_str()
            .expect("Path to metadata should be stringifiable")
    );
    let runtime_path = Path::new(&out_dir).join("runtime.rs");
    fs::write(&runtime_path, runtime_api_contents)
        .expect("Couldn't write runtime rust output");

    let substrate_path =
        which::which(substrate_bin).expect("Cannot resolve path to substrate binary");

    // Re-build if the substrate binary we're pointed to changes (mtime):
    println!(
        "cargo:rerun-if-changed={}",
        substrate_path.to_string_lossy()
    );
    // Re-build if we point to a different substrate binary:
    println!("cargo:rerun-if-env-changed={}", SUBSTRATE_BIN_ENV_VAR);
    // Re-build if this file changes:
    println!("cargo:rerun-if-changed=build.rs");
}

/// Returns the next open port, or None if no port found.
fn next_open_port() -> Option<u16> {
    match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => {
            if let Ok(address) = listener.local_addr() {
                Some(address.port())
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

/// If the substrate process isn't explicitly killed on drop,
/// it seems that panics that occur while the command is running
/// will leave it running and block the build step from ever finishing.
/// Wrapping it in this prevents this from happening.
struct KillOnDrop(std::process::Child);

impl Deref for KillOnDrop {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for KillOnDrop {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

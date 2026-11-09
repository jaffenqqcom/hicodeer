//! Encrypted storage for the IM channels.
//!
//! Credentials never reach settings.json and are never written in clear: the
//! file holds an AES-256-GCM ciphertext, and the key sits in its own
//! owner-only file next to it. Both files live in the app's private data
//! directory.
//!
//! Every read and write goes through one lock, and writes land in a temporary
//! file first, so the bridge thread and the settings page cannot read a
//! half-written file or overwrite each other's change.

use std::fs::OpenOptions;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Context as _;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom as _, SystemRandom};

use crate::config::ImSettings;

const BINDING_FILE: &str = "im_binding.bin";
const KEY_FILE: &str = "im_binding.key";
const KEY_LEN: usize = 32;
/// Owner read/write only.
const FILE_MODE: u32 = 0o600;
/// Suffix of the copy kept when the stored file cannot be read.
const CORRUPT_SUFFIX: &str = "corrupt";

/// Serialises every access to the two files below.
static STORE: Mutex<()> = Mutex::new(());

fn data_path(name: &str) -> PathBuf {
    paths::data_dir().join(name)
}

fn lock_store() -> std::sync::MutexGuard<'static, ()> {
    STORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Returns the key, creating it on first use.
fn load_or_create_key() -> anyhow::Result<Vec<u8>> {
    let path = data_path(KEY_FILE);
    if path.exists() {
        let mut key = Vec::new();
        OpenOptions::new()
            .read(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?
            .read_to_end(&mut key)?;
        anyhow::ensure!(key.len() == KEY_LEN, "im key file has an unexpected length");
        return Ok(key);
    }
    let mut key = vec![0u8; KEY_LEN];
    SystemRandom::new()
        .fill(&mut key)
        .map_err(|_| anyhow::anyhow!("cannot generate an IM storage key"))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .open(&path)
        .with_context(|| format!("create {}", path.display()))?;
    file.write_all(&key)?;
    log::info!("im channel: created a storage key");
    Ok(key)
}

/// Reads the stored settings, or `None` when nothing is stored yet.
pub(crate) fn load() -> anyhow::Result<Option<ImSettings>> {
    let _guard = lock_store();
    load_locked()
}

fn load_locked() -> anyhow::Result<Option<ImSettings>> {
    let path = data_path(BINDING_FILE);
    if !path.exists() {
        return Ok(None);
    }
    match read_settings(&path) {
        Ok(settings) => Ok(Some(settings)),
        Err(error) => {
            // A file that cannot be decrypted or parsed would otherwise make
            // every settings write fail; keep it for diagnosis and start over.
            log::error!("im channel: cannot read the stored configuration: {error:#}");
            let backup = path.with_extension(CORRUPT_SUFFIX);
            if let Err(rename_error) = std::fs::rename(&path, &backup) {
                log::error!(
                    "im channel: cannot move the unreadable configuration to {}: {rename_error}",
                    backup.display()
                );
            }
            Ok(None)
        }
    }
}

fn read_settings(path: &PathBuf) -> anyhow::Result<ImSettings> {
    let mut blob = Vec::new();
    OpenOptions::new()
        .read(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?
        .read_to_end(&mut blob)?;
    anyhow::ensure!(blob.len() > NONCE_LEN, "im binding file is truncated");
    let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
    let key = load_or_create_key()?;
    let opening = LessSafeKey::new(
        UnboundKey::new(&AES_256_GCM, &key).map_err(|_| anyhow::anyhow!("invalid IM key"))?,
    );
    let mut buffer = ciphertext.to_vec();
    let plaintext = opening
        .open_in_place(
            Nonce::try_assume_unique_for_key(nonce)
                .map_err(|_| anyhow::anyhow!("im binding file has an invalid nonce"))?,
            Aad::empty(),
            &mut buffer,
        )
        .map_err(|_| anyhow::anyhow!("im binding file cannot be decrypted"))?;
    serde_json::from_slice(plaintext).context("im binding file is not valid JSON")
}

fn save_locked(settings: &ImSettings) -> anyhow::Result<()> {
    let key = load_or_create_key()?;
    let sealing = LessSafeKey::new(
        UnboundKey::new(&AES_256_GCM, &key).map_err(|_| anyhow::anyhow!("invalid IM key"))?,
    );
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| anyhow::anyhow!("cannot generate a nonce"))?;
    let mut payload = serde_json::to_vec(settings)?;
    sealing
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::empty(),
            &mut payload,
        )
        .map_err(|_| anyhow::anyhow!("cannot encrypt the IM binding"))?;
    let path = data_path(BINDING_FILE);
    let temporary = path.with_extension("bin.tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(FILE_MODE)
        .open(&temporary)
        .with_context(|| format!("write {}", temporary.display()))?;
    file.write_all(&nonce)?;
    file.write_all(&payload)?;
    file.sync_all()?;
    std::fs::rename(&temporary, &path)
        .with_context(|| format!("replace {}", path.display()))?;
    log::info!("im channel: saved the configuration");
    Ok(())
}

/// Reads the stored settings, applies `change` and writes the result back,
/// all under the store lock so a concurrent writer cannot be interleaved.
pub(crate) fn update<F>(change: F) -> anyhow::Result<ImSettings>
where
    F: FnOnce(&mut ImSettings) -> anyhow::Result<()>,
{
    let _guard = lock_store();
    let mut settings = load_locked()?.unwrap_or_default();
    change(&mut settings)?;
    save_locked(&settings)?;
    Ok(settings)
}

//! Stable C ABI for desktop shells (MyGo, Swift, Kotlin or a small Go cgo host).
//! The ABI takes UTF-8 JSON and returns an owned UTF-8 JSON buffer.

use ournotes_search::{
    engine,
    parallel::{self, recommend_json},
};
use ournotes_sim::{cards::Roster, data::DeckData};
use std::{
    ffi::{CStr, CString, c_char},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, OnceLock},
};

/// Admit one CPU search request, which owns a configurable number of internal
/// search workers. Concurrent desktop requests wait for this allowance.
struct SearchPool {
    slots: Mutex<usize>,
    wake: Condvar,
}

impl SearchPool {
    fn new() -> Self {
        // One admitted request owns the native half-core worker allowance.
        Self { slots: Mutex::new(1), wake: Condvar::new() }
    }

    fn acquire(&self) -> SearchPermit<'_> {
        let mut slots = self.slots.lock().expect("search pool mutex poisoned");
        while *slots == 0 {
            slots = self.wake.wait(slots).expect("search pool mutex poisoned");
        }
        *slots -= 1;
        SearchPermit { pool: self }
    }
}

struct SearchPermit<'a> {
    pool: &'a SearchPool,
}

impl Drop for SearchPermit<'_> {
    fn drop(&mut self) {
        let mut slots = self.pool.slots.lock().expect("search pool mutex poisoned");
        *slots += 1;
        self.pool.wake.notify_one();
    }
}

static SEARCH_POOL: OnceLock<Arc<SearchPool>> = OnceLock::new();

fn search_pool() -> &'static Arc<SearchPool> {
    SEARCH_POOL.get_or_init(|| Arc::new(SearchPool::new()))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalManifest {
    #[serde(default = "manifest_format")]
    format: String,
    deck_data: String,
    #[serde(default)]
    assets_dir: Option<String>,
    #[serde(default)]
    account_dir: Option<String>,
}

fn manifest_format() -> String {
    "ournotes.local/1".to_owned()
}

fn resolve_manifest(path: impl AsRef<Path>) -> Result<(PathBuf, serde_json::Value), String> {
    let manifest_path = path.as_ref();
    let text = fs::read_to_string(manifest_path).map_err(|e| format!("read manifest: {e}"))?;
    let manifest: LocalManifest = serde_json::from_str(&text).map_err(|e| format!("manifest JSON: {e}"))?;
    if manifest.format != "ournotes.local/1" {
        return Err(format!("unsupported manifest format {:?}", manifest.format));
    }
    let root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let deck_data = root.join(&manifest.deck_data);
    if !deck_data.is_file() {
        return Err(format!("manifest deckData is not a file: {}", deck_data.display()));
    }
    let assets_dir = manifest.assets_dir.map(|v| root.join(v));
    let account_dir = manifest.account_dir.map(|v| root.join(v));
    let info = serde_json::json!({
        "format": manifest.format,
        "manifest": manifest_path,
        "root": root,
        "deckData": deck_data,
        "assetsDir": assets_dir,
        "accountDir": account_dir,
    });
    Ok((deck_data, info))
}

fn input(ptr: *const c_char) -> Result<String, String> {
    if ptr.is_null() {
        return Err("null input".into());
    }
    // SAFETY: every ABI caller supplies a readable NUL-terminated string that remains
    // valid throughout this call; null is checked above.
    unsafe { CStr::from_ptr(ptr).to_str().map(str::to_owned).map_err(|e| e.to_string()) }
}

fn owned(text: String) -> *mut c_char {
    CString::new(text).unwrap_or_else(|_| CString::new("{\"error\":\"NUL in result\"}").unwrap()).into_raw()
}

fn inspect_local(manifest_path: &str, roster_text: Option<&str>) -> serde_json::Value {
    let mut errors = Vec::new();
    let manifest = match resolve_manifest(manifest_path) {
        Ok((deck_path, info)) => {
            let mut value = info;
            match DeckData::from_path(&deck_path) {
                Ok(data) => {
                    value["dataset"] = serde_json::json!({
                        "format": ournotes_sim::data::FORMAT,
                        "sha256": data.sha256,
                        "chartCount": data.charts.len(),
                        "master": {
                            "memberCards": data.master.member_cards.len(),
                            "supportCards": data.master.support_cards.len(),
                            "characters": data.master.characters.len(),
                            "liveMusics": data.master.live_musics.len(),
                            "liveMusicScores": data.master.live_music_scores.len(),
                        },
                        "provenance": data.provenance,
                    });
                }
                Err(error) => errors.push(serde_json::json!({"path":"deckData", "message": error.to_string()})),
            }
            value
        }
        Err(error) => {
            errors.push(serde_json::json!({"path":"manifest", "message":error}));
            serde_json::json!({"manifest": manifest_path})
        }
    };

    let mut result = serde_json::json!({
        "format": "ournotes.local-summary/1",
        "status": "ok",
        "manifest": manifest,
        "errors": errors,
    });
    if let Some(text) = roster_text {
        match Roster::from_json(text) {
            Ok(roster) => {
                result["account"] = serde_json::json!({
                    "status": "ok",
                    "player": {
                        "vipRank": roster.player.vip_rank,
                        "characterCount": roster.player.character_ranks.len(),
                        "characterTotalRank": roster.player.character_total_rank(),
                        "bandItemCount": roster.player.band_items.len(),
                        "eventCount": roster.player.events.len(),
                    },
                    "members": roster.members.iter().map(|card| serde_json::json!({
                        "id": card.id, "level": card.level, "exp": card.exp,
                        "awake": card.awake, "rank": card.rank,
                        "liveSkillLevel": card.live_skill_level,
                        "gekisouSkillLevel": card.gekisou_skill_level,
                    })).collect::<Vec<_>>(),
                    "snaps": roster.snaps.iter().map(|card| serde_json::json!({
                        "id": card.id, "level": card.level, "exp": card.exp, "rank": card.rank,
                    })).collect::<Vec<_>>(),
                });
            }
            Err(error) => result["errors"].as_array_mut().unwrap().push(serde_json::json!({
                "path":"account", "message":error.to_string()
            })),
        }
    }
    if result["errors"].as_array().is_some_and(|items| !items.is_empty()) {
        result["status"] = serde_json::json!("invalid");
    }
    result
}

/// Loads deck data, evaluates one recommendation, and returns JSON. The caller frees the result with
/// [`ournotes_free_string`]. A null return means invalid pointers or a result containing an interior NUL.
#[unsafe(no_mangle)]
pub extern "C" fn ournotes_recommend_json(
    data_path: *const c_char,
    roster_json: *const c_char,
    request_json: *const c_char,
) -> *mut c_char {
    // SAFETY: the legacy ABI forwards its existing caller pointer contract.
    unsafe {
        ournotes_recommend_json_with_threads(data_path, roster_json, request_json, parallel::default_workers() as u32)
    }
}

/// Same ownership as `ournotes_recommend_json`; threads must be in 1..=logical CPUs.
///
/// # Safety
/// Each non-null pointer must remain readable, NUL-terminated UTF-8 for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ournotes_recommend_json_with_threads(
    data_path: *const c_char,
    roster_json: *const c_char,
    request_json: *const c_char,
    threads: u32,
) -> *mut c_char {
    guarded_json(|| {
        parallel::validate_workers(threads as usize).map_err(|e| e.to_string())?;
        let _permit = search_pool().acquire();
        let data_path = input(data_path)?;
        let roster_json = input(roster_json)?;
        let request_json = input(request_json)?;
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let data = DeckData::from_path(data_path).map_err(|e| e.to_string())?;
                    parallel::recommend_json_with_threads(&data, &roster_json, &request_json, threads as usize)
                        .map_err(|e| e.to_string())
                })
                .join()
                .map_err(|_| "native search worker panicked".to_owned())?
        })
    })
}

// The complete operation, including pool acquisition and dataset parsing, is inside
// the panic boundary. Returned native account answers are serialized unchanged.
fn guarded_json(operation: impl FnOnce() -> Result<String, String>) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation));
    owned(match result {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => serde_json::json!({"error": error}).to_string(),
        Err(_) => serde_json::json!({"error": "native operation panicked"}).to_string(),
    })
}

/// Recommends directly from the original `ournotes.account/1` JSON and the
/// `ournotes-deck.recommendation-request/2` JSON. The return is the engine's
/// `ournotes-deck.account-recommendation/1` document, preserving all statuses,
/// missing facts, errors, exact fractions and result fields.
/// Free the returned allocation with [`ournotes_free_string`].
///
/// # Safety
/// Each non-null argument must point to readable, NUL-terminated UTF-8 bytes
/// which remain valid for the entire call. Null arguments produce JSON errors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ournotes_recommend_account_json(
    data_path: *const c_char,
    account_json: *const c_char,
    request_json: *const c_char,
) -> *mut c_char {
    // SAFETY: the wrapper forwards the caller's documented pointer lifetime contract.
    unsafe {
        ournotes_recommend_account_json_with_threads(
            data_path,
            account_json,
            request_json,
            parallel::default_workers() as u32,
        )
    }
}

/// Same account result and ownership contract, with threads in 1..=logical CPUs.
///
/// # Safety
/// Each non-null pointer must remain readable, NUL-terminated UTF-8 for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ournotes_recommend_account_json_with_threads(
    data_path: *const c_char,
    account_json: *const c_char,
    request_json: *const c_char,
    threads: u32,
) -> *mut c_char {
    guarded_json(|| {
        parallel::validate_workers(threads as usize).map_err(|e| e.to_string())?;
        let _permit = search_pool().acquire();
        let path = input(data_path)?;
        let account = input(account_json)?;
        let request = input(request_json)?;
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let data = DeckData::from_path(path).map_err(|error| error.to_string())?;
                    serde_json::to_string(
                        &parallel::with_native_threads(threads as usize, || {
                            engine::recommend_account(&data, &account, &request, None)
                        })
                        .map_err(|error| error.to_string())?,
                    )
                    .map_err(|error| error.to_string())
                })
                .join()
                .map_err(|_| "native account search worker panicked".to_owned())?
        })
    })
}

/// Returns the host's thread count range and default. Free with `ournotes_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn ournotes_search_threads_json() -> *mut c_char {
    owned(serde_json::json!({"min":1,"max":parallel::max_workers(),"default":parallel::default_workers()}).to_string())
}

/// Returns the native engine capability document. Free it with [`ournotes_free_string`].
#[unsafe(no_mangle)]
pub extern "C" fn ournotes_capabilities_json() -> *mut c_char {
    guarded_json(|| Ok(engine::capabilities().to_string()))
}

/// Returns the exact loaded dataset SHA-256 as `datasetId`, plus `modelCommit`
/// when the dataset provenance declares it. Free it with [`ournotes_free_string`].
///
/// # Safety
/// `data_path` must be null or point to readable, NUL-terminated UTF-8 bytes
/// which remain valid for the entire call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ournotes_dataset_info_json(data_path: *const c_char) -> *mut c_char {
    guarded_json(|| {
        let path = input(data_path)?;
        let data = DeckData::from_path(path).map_err(|error| error.to_string())?;
        let commit =
            data.provenance.get("deck").and_then(|deck| deck.get("commit")).and_then(serde_json::Value::as_str);
        Ok(serde_json::json!({"datasetId": data.sha256, "modelCommit": commit}).to_string())
    })
}

/// Loads the deck path from an `ournotes.local/1` manifest and evaluates one recommendation.
/// The caller frees the result with [`ournotes_free_string`].
#[unsafe(no_mangle)]
pub extern "C" fn ournotes_recommend_manifest_json(
    manifest_path: *const c_char,
    roster_json: *const c_char,
    request_json: *const c_char,
) -> *mut c_char {
    let result = (|| {
        let _permit = search_pool().acquire();
        let manifest = input(manifest_path)?;
        let roster_json = input(roster_json)?;
        let request_json = input(request_json)?;
        let (deck_data, _) = resolve_manifest(&manifest)?;
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let data = DeckData::from_path(deck_data).map_err(|e| e.to_string())?;
                    recommend_json(&data, &roster_json, &request_json).map_err(|e| e.to_string())
                })
                .join()
                .map_err(|_| "native search worker panicked".to_owned())?
        })
    })();
    owned(match result {
        Ok(value) => value,
        Err(error) => serde_json::json!({"error":error}).to_string(),
    })
}

/// Resolves a local resource manifest and returns its paths as JSON. This lets a desktop shell
/// select one local data directory once and pass the resulting paths to its UI.
#[unsafe(no_mangle)]
pub extern "C" fn ournotes_manifest_json(manifest_path: *const c_char) -> *mut c_char {
    let result = (|| {
        let path = input(manifest_path)?;
        let (_, info) = resolve_manifest(path)?;
        serde_json::to_string(&info).map_err(|e| e.to_string())
    })();
    owned(match result {
        Ok(value) => value,
        Err(error) => serde_json::json!({"error":error}).to_string(),
    })
}

/// Inspects a local manifest and optional roster JSON for a desktop UI. The returned document is
/// `ournotes.local-summary/1`; malformed inputs are represented in `errors` instead of aborting the process.
/// Pass a null roster pointer to inspect only the manifest and deck data.
#[unsafe(no_mangle)]
pub extern "C" fn ournotes_local_summary_json(manifest_path: *const c_char, roster_json: *const c_char) -> *mut c_char {
    let result = (|| {
        let manifest = input(manifest_path)?;
        let roster = if roster_json.is_null() { None } else { Some(input(roster_json)?) };
        Ok::<_, String>(inspect_local(&manifest, roster.as_deref()).to_string())
    })();
    owned(result.unwrap_or_else(|error| serde_json::json!({"format":"ournotes.local-summary/1", "status":"invalid", "errors":[{"path":"input", "message":error}]}).to_string()))
}

/// Returns the byte length of an owned JSON string, excluding its terminating NUL.
/// A null pointer returns zero. The allocation remains owned by the caller and
/// must eventually be returned to [`ournotes_free_string`].
///
/// # Safety
/// A non-null pointer must point to a live, NUL-terminated string returned by
/// this library. It must remain valid and unmodified throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ournotes_string_len(value: *const c_char) -> usize {
    if value.is_null() {
        return 0;
    }
    // SAFETY: the caller supplies a live NUL-terminated allocation from this ABI.
    unsafe { CStr::from_ptr(value) }.to_bytes().len()
}

/// Frees a string returned by this library. Passing null is allowed.
#[unsafe(no_mangle)]
pub extern "C" fn ournotes_free_string(value: *mut c_char) {
    if !value.is_null() {
        // SAFETY: the ABI caller returns an unmodified allocation from owned(), once.
        unsafe {
            drop(CString::from_raw(value));
        }
    }
}

/// Returns the ABI version for host compatibility checks.
#[unsafe(no_mangle)]
pub extern "C" fn ournotes_ffi_version() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn take_json(ptr: *mut c_char) -> serde_json::Value {
        assert!(!ptr.is_null());
        // SAFETY: ptr is the owned ABI result and is freed only after this call.
        let len = unsafe { ournotes_string_len(ptr) };
        // SAFETY: len is the exact initialized byte length of that allocation.
        let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) };
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(bytes).unwrap(),
            serde_json::from_str::<serde_json::Value>(unsafe { CStr::from_ptr(ptr) }.to_str().unwrap()).unwrap()
        );
        // SAFETY: each test receives an owned NUL-terminated ABI allocation and
        // copies its bytes before freeing it exactly once.
        let text = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_owned();
        ournotes_free_string(ptr);
        serde_json::from_str(&text).unwrap()
    }

    fn dataset_fixture() -> (PathBuf, DeckData) {
        let path = std::env::temp_dir().join(format!(
            "ournotes-ffi-account-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        let tables: serde_json::Map<String, serde_json::Value> = ournotes_sim::master::TABLES
            .iter()
            .map(|name| ((*name).to_owned(), serde_json::json!({"columns": [], "rows": []})))
            .collect();
        let text = serde_json::json!({"format": ournotes_sim::data::FORMAT,
            "provenance": {"region": "jp", "deck": {"commit": "synthetic-test"}},
            "master": tables, "charts": []})
        .to_string();
        fs::write(&path, text).unwrap();
        let data = DeckData::from_path(&path).unwrap();
        (path, data)
    }

    #[test]
    fn account_abi_preserves_native_answer_for_valid_and_invalid_json() {
        let (path, data) = dataset_fixture();
        let coverage: serde_json::Map<String, serde_json::Value> = [
            "_player._memberCards",
            "_player._supportCards",
            "_player._characters",
            "_player._bandItems",
            "_player._memory._musicGroups",
            "_player._memory._members",
            "_player._memory._supports",
        ]
        .into_iter()
        .map(|key| (key.into(), serde_json::json!("partial")))
        .collect();
        let account = serde_json::json!({"format": "ournotes.account/1", "datasetId": data.sha256,
            "server": "jp", "revision": "synthetic", "coverage": coverage, "assumptions": [],
            "declared": {}, "account": {"_player": {}}})
        .to_string();
        ournotes_sim::account::AccountInput::from_json(&account).unwrap();
        let request = r#"{"format":"ournotes-deck.recommendation-request/2","goal":{"kind":"power"},"k":1}"#;
        let path_c = CString::new(path.to_str().unwrap()).unwrap();
        for account_text in [account.as_str(), "{malformed", "{}"] {
            let account_c = CString::new(account_text).unwrap();
            let request_c = CString::new(request).unwrap();
            // SAFETY: all three CStrings live across the complete call.
            let actual = take_json(unsafe {
                ournotes_recommend_account_json(path_c.as_ptr(), account_c.as_ptr(), request_c.as_ptr())
            });
            let expected = serde_json::to_value(engine::recommend_account(&data, account_text, request, None)).unwrap();
            assert_eq!(actual, expected);
            assert_eq!(actual["format"], "ournotes-deck.account-recommendation/1");
            assert_eq!(actual["final"], true);
        }
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn abi_string_length_is_exact_for_large_utf8_output_and_null() {
        // SAFETY: null is explicitly supported.
        assert_eq!(unsafe { ournotes_string_len(std::ptr::null()) }, 0);
        let text = "卡".repeat(400_000);
        let ptr = owned(text.clone());
        // SAFETY: ptr points to a live ABI-owned NUL-terminated allocation.
        assert_eq!(unsafe { ournotes_string_len(ptr) }, text.len());
        ournotes_free_string(ptr);
    }

    #[test]
    fn capabilities_dataset_and_transport_errors_are_json() {
        assert_eq!(take_json(ournotes_capabilities_json()), engine::capabilities());
        let (path, data) = dataset_fixture();
        let path_c = CString::new(path.to_str().unwrap()).unwrap();
        // SAFETY: the path CString lives for the call; null is supported by the ABI.
        let info = take_json(unsafe { ournotes_dataset_info_json(path_c.as_ptr()) });
        assert_eq!(info["datasetId"], data.sha256.unwrap());
        assert_eq!(info["modelCommit"], "synthetic-test");
        let null = std::ptr::null();
        // SAFETY: the ABI handles null arguments before dereferencing them.
        assert!(take_json(unsafe { ournotes_recommend_account_json(null, null, null) })["error"].is_string());
        // SAFETY: null is explicitly supported as an invalid input.
        assert!(take_json(unsafe { ournotes_dataset_info_json(null) })["error"].is_string());
        assert!(take_json(guarded_json(|| panic!("synthetic panic")))["error"].is_string());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn thread_parameter_metadata_and_abi_range_errors() {
        let counts = take_json(ournotes_search_threads_json());
        assert_eq!(counts["min"], 1);
        assert_eq!(counts["max"], parallel::max_workers());
        assert_eq!(counts["default"], parallel::default_workers());
        let null = std::ptr::null();
        for count in [0, parallel::max_workers() as u32 + 1] {
            // SAFETY: these functions reject thread counts before reading arguments;
            // null is also explicitly handled by the ABI input checks.
            let roster = take_json(unsafe { ournotes_recommend_json_with_threads(null, null, null, count) });
            let account = take_json(unsafe { ournotes_recommend_account_json_with_threads(null, null, null, count) });
            assert!(roster["error"].as_str().unwrap().contains("workers must be in"));
            assert_eq!(roster, account);
        }
    }

    #[test]
    fn resolves_manifest_relative_paths() {
        let root = std::env::temp_dir().join(format!("ournotes-ffi-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::create_dir_all(root.join("nnnotes.deck-data")).unwrap();
        fs::write(root.join("nnnotes.deck-data/1"), b"fixture").unwrap();
        let manifest = root.join("manifest.json");
        fs::write(&manifest, r#"{"format":"ournotes.local/1","deckData":"nnnotes.deck-data/1","assetsDir":"assets"}"#)
            .unwrap();

        let (_, info) = resolve_manifest(&manifest).unwrap();
        assert_eq!(info["format"], "ournotes.local/1");
        assert!(info["deckData"].as_str().unwrap().ends_with("nnnotes.deck-data/1"));
        assert!(info["assetsDir"].as_str().unwrap().ends_with("assets"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_unknown_manifest_format() {
        let root = std::env::temp_dir().join(format!("ournotes-ffi-bad-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let manifest = root.join("manifest.json");
        fs::write(&manifest, r#"{"format":"other/1","deckData":"data"}"#).unwrap();
        let error = resolve_manifest(&manifest).unwrap_err();
        assert!(error.contains("unsupported manifest format"));
        let _ = fs::remove_dir_all(root);
    }
}

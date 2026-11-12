//! The launcher's platform half (`swift/Platform.swift`): links open in Safari, the sign-in code
//! goes on the clipboard, files come in through the document picker and leave through the share
//! sheet, and the Microsoft account's tokens live in the Keychain.
#![allow(unsafe_code)]

use horizon_client::launcher_ui::Platform;
use horizon_client::msa::SecretStore;
use std::ffi::{CStr, CString, c_char};
use std::path::{Path, PathBuf};

unsafe extern "C" {
    fn horizon_free_string(text: *mut c_char);
    fn horizon_open_url(text: *const c_char);
    fn horizon_copy_text(text: *const c_char);
    fn horizon_pick_files();
    fn horizon_take_picked() -> *mut c_char;
    fn horizon_share_file(path: *const c_char);
    fn horizon_keychain_get(name: *const c_char) -> *mut c_char;
    fn horizon_keychain_set(name: *const c_char, value: *const c_char) -> i32;
    fn horizon_keychain_delete(name: *const c_char);
}

// SAFETY (all below): the strings passed are live `CString`s; a string Swift returns is malloc'd
// by it (`strdup`) or null, copied here and freed once with `horizon_free_string`. The UIKit calls
// run on the main thread, where winit's event loop runs the launcher.

fn take(text: *mut c_char) -> Option<String> {
    if text.is_null() {
        return None;
    }
    let owned = unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned();
    unsafe { horizon_free_string(text) };
    Some(owned)
}

pub struct Ios;

impl Platform for Ios {
    fn open_url(&self, url: &str) {
        if let Ok(url) = CString::new(url) {
            unsafe { horizon_open_url(url.as_ptr()) }
        }
    }

    fn copy_text(&self, text: &str) {
        if let Ok(text) = CString::new(text) {
            unsafe { horizon_copy_text(text.as_ptr()) }
        }
    }

    fn pick_files(&self) {
        unsafe { horizon_pick_files() }
    }

    fn take_picked(&self) -> Vec<PathBuf> {
        take(unsafe { horizon_take_picked() })
            .map(|paths| paths.lines().map(PathBuf::from).collect())
            .unwrap_or_default()
    }

    fn share_file(&self, path: &Path) {
        if let Ok(path) = CString::new(path.as_os_str().as_encoded_bytes()) {
            unsafe { horizon_share_file(path.as_ptr()) }
        }
    }
}

/// The account's secrets as one Keychain item.
pub struct Keychain;

const ITEM: &CStr = c"microsoft";

impl SecretStore for Keychain {
    fn get(&self) -> Option<String> {
        take(unsafe { horizon_keychain_get(ITEM.as_ptr()) })
    }

    fn set(&self, value: &str) -> Result<(), String> {
        let value = CString::new(value).map_err(|e| e.to_string())?;
        match unsafe { horizon_keychain_set(ITEM.as_ptr(), value.as_ptr()) } {
            0 => Ok(()),
            status => Err(format!("Keychain error {status}")),
        }
    }

    fn delete(&self) {
        unsafe { horizon_keychain_delete(ITEM.as_ptr()) }
    }
}

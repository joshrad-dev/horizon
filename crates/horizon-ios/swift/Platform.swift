// The launcher's platform half on iOS (`launcher_ui::Platform`, `msa::SecretStore`): links in
// Safari, the clipboard, the document picker for files to bring in, the share sheet, and the
// Keychain for the Microsoft account's tokens. The Rust app calls the C functions at the bottom
// (src/platform.rs) on the main thread; strings it gets back are malloc'd and freed with
// `horizon_free_string`.
import Foundation
import Security
import UIKit
import UniformTypeIdentifiers

/// The view controller on top, to present the picker and the share sheet from.
@MainActor
private func topController() -> UIViewController? {
    let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
    let window = scenes.flatMap(\.windows).first(where: \.isKeyWindow) ?? scenes.first?.windows.first
    var top = window?.rootViewController
    while let presented = top?.presentedViewController {
        top = presented
    }
    return top
}

/// The files the player picked, until the game takes them.
private let pickedLock = NSLock()
nonisolated(unsafe) private var picked: [String] = []

@MainActor
private final class PickerDelegate: NSObject, UIDocumentPickerDelegate {
    static let shared = PickerDelegate()

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        // `asCopy`: the picker already copied them into the app's own space.
        pickedLock.lock()
        picked.append(contentsOf: urls.map(\.path))
        pickedLock.unlock()
    }
}

/// On iPad the share sheet is a popover: it points at the screen's bottom middle.
@MainActor
private func anchor(_ controller: UIViewController, in top: UIViewController) {
    if let popover = controller.popoverPresentationController {
        popover.sourceView = top.view
        popover.sourceRect = CGRect(x: top.view.bounds.midX, y: top.view.bounds.maxY - 8, width: 1, height: 1)
        popover.permittedArrowDirections = [.down]
    }
}

private func string(_ text: UnsafePointer<CChar>?) -> String? {
    text.map { String(cString: $0) }
}

private func owned(_ text: String) -> UnsafeMutablePointer<CChar>? {
    strdup(text)
}

@_cdecl("horizon_free_string")
public func horizonFreeString(_ text: UnsafeMutablePointer<CChar>?) {
    free(text)
}

@_cdecl("horizon_open_url")
public func horizonOpenUrl(_ text: UnsafePointer<CChar>?) {
    guard let text = string(text), let url = URL(string: text) else { return }
    MainActor.assumeIsolated {
        UIApplication.shared.open(url)
    }
}

@_cdecl("horizon_copy_text")
public func horizonCopyText(_ text: UnsafePointer<CChar>?) {
    guard let text = string(text) else { return }
    UIPasteboard.general.string = text
}

@_cdecl("horizon_pick_files")
public func horizonPickFiles() {
    MainActor.assumeIsolated {
        guard let top = topController() else { return }
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.item], asCopy: true)
        picker.allowsMultipleSelection = true
        picker.delegate = PickerDelegate.shared
        top.present(picker, animated: true)
    }
}

/// The picked files' paths, one per line, or null when there are none.
@_cdecl("horizon_take_picked")
public func horizonTakePicked() -> UnsafeMutablePointer<CChar>? {
    pickedLock.lock()
    let paths = picked
    picked.removeAll()
    pickedLock.unlock()
    return paths.isEmpty ? nil : owned(paths.joined(separator: "\n"))
}

@_cdecl("horizon_share_file")
public func horizonShareFile(_ text: UnsafePointer<CChar>?) {
    guard let path = string(text) else { return }
    MainActor.assumeIsolated {
        guard let top = topController() else { return }
        let sheet = UIActivityViewController(activityItems: [URL(fileURLWithPath: path)], applicationActivities: nil)
        anchor(sheet, in: top)
        top.present(sheet, animated: true)
    }
}

// The Keychain: one generic password, this device only, readable after the first unlock (the
// game may renew the session while the phone is locked in a pocket).
private func keychainQuery(_ name: String) -> [String: Any] {
    [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: "horizon.account",
        kSecAttrAccount as String: name,
    ]
}

@_cdecl("horizon_keychain_get")
public func horizonKeychainGet(_ name: UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>? {
    guard let name = string(name) else { return nil }
    var query = keychainQuery(name)
    query[kSecReturnData as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne
    var result: CFTypeRef?
    guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
          let data = result as? Data,
          let text = String(data: data, encoding: .utf8)
    else { return nil }
    return owned(text)
}

/// 0 when stored, else the Keychain's status.
@_cdecl("horizon_keychain_set")
public func horizonKeychainSet(_ name: UnsafePointer<CChar>?, _ value: UnsafePointer<CChar>?) -> Int32 {
    guard let name = string(name), let value = string(value) else { return -1 }
    let data = Data(value.utf8)
    let query = keychainQuery(name)
    let update: [String: Any] = [
        kSecValueData as String: data,
        kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
    ]
    var status = SecItemUpdate(query as CFDictionary, update as CFDictionary)
    if status == errSecItemNotFound {
        var add = query
        add.merge(update) { _, new in new }
        status = SecItemAdd(add as CFDictionary, nil)
    }
    return status
}

@_cdecl("horizon_keychain_delete")
public func horizonKeychainDelete(_ name: UnsafePointer<CChar>?) {
    guard let name = string(name) else { return }
    SecItemDelete(keychainQuery(name) as CFDictionary)
}

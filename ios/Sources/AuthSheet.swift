// Spotify's sign-in pages in a Safari sheet over the app, for the Rust side
// (src/ios/auth_sheet.rs). The sheet keeps Spotifast in the foreground, so
// the loopback listener that waits for Spotify's redirect stays alive.

import SafariServices
import UIKit

/// The one sign-in sheet, over Spotifast's window.
final class AuthSheet: NSObject, SFSafariViewControllerDelegate {
    static let shared = AuthSheet()

    private var sheet: SFSafariViewController?

    /// Shows `url`, in place of a sheet already shown.
    func present(_ url: URL) {
        guard let shown = sheet else {
            show(url, attempt: 0)
            return
        }
        sheet = nil
        shown.dismiss(animated: false) {
            self.show(url, attempt: 0)
        }
    }

    /// Takes the sheet away, if one is shown.
    func dismiss() {
        guard let shown = sheet else { return }
        sheet = nil
        shown.dismiss(animated: true)
        report("sign-in sheet: taken away")
    }

    private func show(_ url: URL, attempt: Int) {
        guard let host = Self.topViewController() else {
            // The window is not there yet: try again shortly.
            if attempt < 20 {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) {
                    self.show(url, attempt: attempt + 1)
                }
            } else {
                report("sign-in sheet: no window to show it over")
            }
            return
        }
        let configuration = SFSafariViewController.Configuration()
        configuration.entersReaderIfAvailable = false
        let controller = SFSafariViewController(url: url, configuration: configuration)
        controller.delegate = self
        controller.dismissButtonStyle = .cancel
        controller.modalPresentationStyle = .pageSheet
        // Only Cancel closes it, so a closed sheet always reaches the Rust
        // side and gives the sign-in up.
        controller.isModalInPresentation = true
        sheet = controller
        host.present(controller, animated: true)
        report("sign-in sheet: shown")
    }

    func safariViewControllerDidFinish(_ controller: SFSafariViewController) {
        guard controller === sheet else { return }
        sheet = nil
        report("sign-in sheet: closed by the person")
        spotifast_auth_sheet_closed()
    }

    func safariViewController(
        _ controller: SFSafariViewController, didCompleteInitialLoad didLoadSuccessfully: Bool
    ) {
        report("sign-in sheet: first page " + (didLoadSuccessfully ? "loaded" : "did not load"))
    }

    func safariViewController(
        _ controller: SFSafariViewController, initialLoadDidRedirectTo URL: URL
    ) {
        report("sign-in sheet: redirected to " + (URL.host ?? "an address without a host"))
    }

    /// The view controller on top of Spotifast's window.
    private static func topViewController() -> UIViewController? {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        let window = scenes.compactMap(\.keyWindow).first ?? scenes.flatMap(\.windows).first
        var top = window?.rootViewController
        while let presented = top?.presentedViewController {
            top = presented
        }
        return top
    }
}

/// Rust: show a sign-in page. Any thread; the string is copied before this
/// returns.
@_cdecl("spotifast_ios_open_auth_url")
func spotifastIosOpenAuthUrl(_ url: UnsafePointer<CChar>) {
    let text = String(cString: url)
    DispatchQueue.main.async {
        guard let url = URL(string: text) else {
            report("sign-in sheet: an address URL cannot read")
            return
        }
        AuthSheet.shared.present(url)
    }
}

/// Rust: the browser part of a sign-in is over. Any thread.
@_cdecl("spotifast_ios_auth_finished")
func spotifastIosAuthFinished() {
    DispatchQueue.main.async {
        AuthSheet.shared.dismiss()
    }
}

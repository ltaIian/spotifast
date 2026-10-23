// Spotifast's iOS app, as top-level code: no @main, no app delegate and no
// storyboard. winit's event loop, inside spotifast_main, calls
// UIApplicationMain itself and stops if a UIApplication already exists, so
// nothing here may touch UIApplication.shared.

import AVFoundation
import Foundation
import UIKit

/// Whether the simulator's smoke test asked for diagnostics.
let selfTest = ProcessInfo.processInfo.environment["SPOTIFAST_SELFTEST"] == "1"

/// A line for the console and the system log, before Spotifast's own log
/// has started.
func report(_ line: String) {
    print("Spotifast: \(line)")
    NSLog("Spotifast: %@", line)
}

// jiff takes the zone from TZ and finds it in its own database.
setenv("TZ", TimeZone.current.identifier, 1)

// The audio session cpal takes the output from: playback, which goes on with
// the screen locked and the silent switch on, at Spotify's 44.1 kHz so that
// nothing is resampled, and active before the player opens its output.
let session = AVAudioSession.sharedInstance()
do {
    try session.setCategory(.playback, mode: .default)
} catch {
    report("audio session category: \(error)")
}
do {
    try session.setPreferredSampleRate(44_100)
} catch {
    report("audio session sample rate: \(error)")
}
do {
    try session.setActive(true)
} catch {
    report("audio session activation: \(error)")
}
if selfTest {
    let route = session.currentRoute.outputs.map { $0.portType.rawValue }.joined(separator: ", ")
    report(
        "selftest: audio session: \(session.category.rawValue), \(session.mode.rawValue), "
            + "\(session.sampleRate) Hz (\(session.preferredSampleRate) Hz preferred), "
            + "\(session.outputNumberOfChannels) output channels, route [\(route)]"
    )
}

// iOS may suspend the app once it is in the background, and end it without
// warning: Spotifast saves then, as it does on the desktop when it quits.
// These come on the main thread, and the saving is done before they return.
let notifications = NotificationCenter.default
_ = notifications.addObserver(
    forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: nil
) { _ in
    spotifast_will_background()
}
_ = notifications.addObserver(
    forName: UIApplication.willTerminateNotification, object: nil, queue: nil
) { _ in
    spotifast_will_terminate()
}

spotifast_main()

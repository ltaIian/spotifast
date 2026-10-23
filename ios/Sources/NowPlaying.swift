// Now Playing on the lock screen and in Control Center, and the remote
// commands they and headphones send back, for the Rust side
// (src/ios/now_playing.rs).

import MediaPlayer
import UIKit

enum NowPlaying {
    /// The codes spotifast_ios_command takes, as in src/ios/now_playing.rs.
    private enum Code: Int32 {
        case play = 1
        case pause = 2
        case playPause = 3
        case next = 4
        case previous = 5
        /// Move to a position, in seconds from the start of the track.
        case setPosition = 6
    }

    /// Lines for the simulator's smoke test.
    private static let verbose = ProcessInfo.processInfo.environment["SPOTIFAST_SELFTEST"] != nil

    /// The cover last shown, by the file it came from.
    private static var cover: (path: String, artwork: MPMediaItemArtwork)?

    /// Turns the remote commands on.
    static func start() {
        let center = MPRemoteCommandCenter.shared()
        _ = center.playCommand.addTarget { _ in send(.play) }
        _ = center.pauseCommand.addTarget { _ in send(.pause) }
        _ = center.togglePlayPauseCommand.addTarget { _ in send(.playPause) }
        _ = center.nextTrackCommand.addTarget { _ in send(.next) }
        _ = center.previousTrackCommand.addTarget { _ in send(.previous) }
        _ = center.changePlaybackPositionCommand.addTarget { event in
            guard let event = event as? MPChangePlaybackPositionCommandEvent else {
                return .commandFailed
            }
            return send(.setPosition, event.positionTime)
        }
        if verbose {
            report("now playing: remote commands on")
        }
    }

    /// Shows a track. iOS moves the elapsed time on by itself at rate 1.
    static func show(
        title: String, artist: String, album: String, artwork path: String,
        duration: Double, position: Double, playing: Bool
    ) {
        var info: [String: Any] = [
            MPMediaItemPropertyTitle: title,
            MPMediaItemPropertyArtist: artist,
            MPMediaItemPropertyAlbumTitle: album,
            MPNowPlayingInfoPropertyElapsedPlaybackTime: position,
            MPNowPlayingInfoPropertyPlaybackRate: playing ? 1.0 : 0.0,
            MPNowPlayingInfoPropertyDefaultPlaybackRate: 1.0,
            MPNowPlayingInfoPropertyMediaType: MPNowPlayingInfoMediaType.audio.rawValue,
        ]
        if duration > 0 {
            info[MPMediaItemPropertyPlaybackDuration] = duration
        }
        if let artwork = artwork(at: path) {
            info[MPMediaItemPropertyArtwork] = artwork
        }
        MPNowPlayingInfoCenter.default().nowPlayingInfo = info
        if verbose {
            let shown = MPNowPlayingInfoCenter.default().nowPlayingInfo?.count ?? 0
            report("now playing: " + (playing ? "playing" : "paused") + ", \(shown) details shown")
        }
    }

    /// Shows nothing.
    static func clear() {
        MPNowPlayingInfoCenter.default().nowPlayingInfo = nil
        if verbose {
            report("now playing: nothing shown")
        }
    }

    private static func artwork(at path: String) -> MPMediaItemArtwork? {
        guard !path.isEmpty else { return nil }
        if let cover, cover.path == path {
            return cover.artwork
        }
        guard let image = UIImage(contentsOfFile: path) else { return nil }
        let artwork = MPMediaItemArtwork(boundsSize: image.size) { _ in image }
        cover = (path, artwork)
        return artwork
    }

    private static func send(_ code: Code, _ value: Double = 0) -> MPRemoteCommandHandlerStatus {
        spotifast_ios_command(code.rawValue, value)
        return .success
    }
}

/// Runs `work` on the main thread: at once when already there.
private func onMain(_ work: @escaping () -> Void) {
    if Thread.isMainThread {
        work()
    } else {
        DispatchQueue.main.async(execute: work)
    }
}

/// Rust: turn the remote commands on.
@_cdecl("spotifast_ios_now_playing_start")
func spotifastIosNowPlayingStart() {
    onMain { NowPlaying.start() }
}

/// Rust: show a track. The strings are copied before this returns.
@_cdecl("spotifast_ios_now_playing_update")
func spotifastIosNowPlayingUpdate(
    _ title: UnsafePointer<CChar>, _ artist: UnsafePointer<CChar>,
    _ album: UnsafePointer<CChar>, _ artwork: UnsafePointer<CChar>,
    _ duration: Double, _ position: Double, _ playing: Bool
) {
    let title = String(cString: title)
    let artist = String(cString: artist)
    let album = String(cString: album)
    let artwork = String(cString: artwork)
    onMain {
        NowPlaying.show(
            title: title, artist: artist, album: album, artwork: artwork,
            duration: duration, position: position, playing: playing)
    }
}

/// Rust: show nothing.
@_cdecl("spotifast_ios_now_playing_clear")
func spotifastIosNowPlayingClear() {
    onMain { NowPlaying.clear() }
}

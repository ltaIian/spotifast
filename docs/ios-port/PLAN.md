# Spotifast on iOS: feasibility audit and port plan

Phase 1 output for the `ios-port` branch (cut from tag v0.14.1). Every agent working on Phases 2 to 4 reads this file first, then `AGENTS.md`, then `EQ_INTERFACE.md` if it touches the equaliser.

Date: 2026-10-23. Target: `aarch64-apple-ios` (device) and `aarch64-apple-ios-sim` (simulator), minimum iOS 16, unsigned `.ipa` for AltStore / SideStore / Sideloadly.

## 0. How this was established

Two sources, in order of authority:

1. **A real compile probe.** `.github/workflows/ios-probe.yml` on the fork `ltaIian/spotifast` runs `cargo check --locked --keep-going --no-default-features --lib --bins` for the three targets on a GitHub `macos-latest` runner with the pinned Rust 1.98.0. Run 39899254862 (commit e44ef8c) is the ground truth below. The darwin control compiles clean.
2. **A source audit** of Spotifast and the pinned forks (fastframe v0.14.0, crmne/librespot d047290, crmne/egui 6ce633b, crmne/winit 0bed9dc, crmne/cpal fcd8b6e) by eight parallel auditors, each followed by a completeness critic, with every "blocker" or "needs replacement" claim re-checked by two independent skeptics. 102 agent runs in total. Where the probe and the audit disagreed, the probe wins.

The development machine for this port is Windows with no Rust toolchain, no C++ compiler and no Xcode. **Every iOS compile, simulator build and `.ipa` build runs on the fork's GitHub Actions macOS runner.** See section 9.

## 1. Compile probe result (the ground truth)

`cargo check --target aarch64-apple-ios --no-default-features --lib --bins` over about 430 crates:

| Outcome | Crates |
| --- | --- |
| Fail to compile | `glutin` 0.32.3 (39 errors: no OpenGL backend for iOS, `compile_error!`), `rfd` 0.17.2 (12 errors: no file-dialog backend for iOS) |
| Skipped because they sit downstream of those two | `eframe`, `fastframe-shell`, `fastframe-titlebar`, `spotifast` itself |
| Compile for iOS | everything else, including every librespot crate, reqwest and the rustls stack, hyper-proxy2, the cpal fork (its iOS CoreAudio host, `objc2-avf-audio`), `fastframe-audio`, `fastframe-now-playing`, `fastframe-tray`, `fastframe-update`, `fastframe-instance`, `fastframe-fonts`, `fastframe-emoji`, `keyring-core`, `mdns-sd`, `jiff`, `ring`, `sysinfo` (links IOKit), `objc2-ui-kit` (winit's iOS backend) |

The simulator target fails and succeeds on exactly the same crates as the device target.

Because `spotifast` itself was skipped, its own code has not been type-checked for iOS yet. The audit found exactly one hard error inside it (section 3.5, the keyring arm) and a handful of `eframe::glow` references in the binary-only `entrypoint.rs`. Everything else in `src/` has a fallback arm for "other unix".

## 2. Architecture decisions for the port

These are the orchestrator's decisions. They are consistent with the audit and with `AGENTS.md` (platform code behind `cfg`, desktop keeps compiling, settings files stay compatible). Agents follow them; an agent that needs to deviate stops and reports.

1. **Renderer: wgpu (Metal) on iOS, glow everywhere else.** eframe's `glow` feature pulls glutin, which cannot build for iOS. The egui fork's eframe has an explicit iOS path (`run_and_return` compiled out, `run_native` never returns, the app-creator closure runs inside UIKit's `resumed`). Cargo features in `[target.'cfg(...)'.dependencies]` apply only to that target, so Spotifast's `eframe` line moves into two target tables: desktop keeps `glow, wayland, x11`, iOS gets `wgpu` (prefer `wgpu_no_default_features` plus `wgpu = { default-features = false, features = ["metal", "wgsl", "std"] }` to skip Vulkan/GLES code; fall back to plain `wgpu` if feature resolution fights back). `accesskit`, `default_fonts`, `links`, `persistence` stay in the unconditional entry. `links` is what makes `ctx.open_url` work on iOS.
2. **fastframe-shell and fastframe-titlebar leave the iOS dependency graph.** Both carry fastframe's workspace `eframe` dependency with `glow`, which would drag glutin back in. Put them in `[target.'cfg(not(target_os = "ios"))'.dependencies]` and shim their call sites (`Shell::run` loop in `entrypoint.rs`; `App`'s `Resident` impl and `Geometry` in `app.rs`/`winamp.rs`; about 20 titlebar calls in `app.rs`, `entrypoint.rs`, `theme.rs`, `ui/login.rs`, `ui/mod.rs`, `ui/topbar.rs`) with `cfg(not(target_os = "ios"))` or zero-returning stubs. On iOS the title bar is inert anyway.
3. **Static library: a new workspace member `ios/` (`spotifast-ios`, `crate-type = ["staticlib"]`)** depending on `spotifast = { path = "..", default-features = false }`. Adding `staticlib` to the root `[lib]` would make every desktop build archive the whole graph. The root `Cargo.toml` gains `[workspace] members = ["ios"]`. **Mandatory in the same change:** `ci.yml` lines 171, 194 and 219 read `cargo metadata --no-deps ... packages[0]`, and with a workspace member that index becomes the member, so select the package by name `spotifast` there.
4. **Entry point owned by Rust.** winit's iOS `EventLoop::run_app` is what calls `UIApplicationMain`, and it asserts no `UIApplication` exists yet. The Xcode target therefore has **no `@main`, no `UIApplicationDelegate`, no storyboard**: a `main.swift` with top-level code that configures `AVAudioSession`, registers the Swift callbacks (section 4) and calls `spotifast_main()`, which never returns. `spotifast_main` lives in `src/ios/mod.rs` behind `#[cfg(target_os = "ios")] pub mod ios;` in `lib.rs`, as `#[unsafe(no_mangle)] pub extern "C"`. It reproduces what `entrypoint.rs` and `fastframe_shell::Shell::run` do for the app: `AppDirs::discover`, `dirs.ensure()`, `fastframe_log::Logging` into the sandbox, `Settings::load`, `fastframe_file::mark_ui_thread()`, `App::new` with `AppOptions { restore_sign_in: true, media_controls: false, tray: false, autostart: false }`, `waker.attach(&cc.egui_ctx)`, then one `eframe::run_native` with `persistence_path: Some(<Library/Application Support/spotifast/app.ron>)` (eframe cannot derive a path on iOS) and its own small `eframe::App` wrapper that owns `app::App` directly (fastframe-shell's `Held<App>` cannot be constructed outside `Shell::run`). `on_exit` never runs on iOS, so a `spotifast_will_background()` hook flushes files (`App::flush_files`) from `applicationDidEnterBackground`. The desktop `entrypoint.rs` is untouched apart from `cfg` gates around `eframe::glow` (lines 647 to 661, 1573).
5. **TLS roots: webpki on iOS.** `rustls-native-certs` returns an empty store on iOS (it probes `/etc/ssl` paths that do not exist), so librespot's hyper client aborts at `http_client.rs:179` (`panic = "abort"`), hyper-proxy2's dummy `ProxyConnector` fails even with no proxy, the dealer WebSocket returns `NotFound`, and every reqwest request fails validation. Fix: move `librespot-core`, `librespot-playback` and `reqwest` into two target tables; desktop unchanged, iOS with `rustls-tls-webpki-roots` (and native roots **off** for librespot on iOS; both features on one target does not help because `with_native_roots()?` still runs). hyper-proxy2 and tokio-tungstenite follow through librespot-core's feature. First simulator smoke test: a login or apresolve round trip.
6. **Credentials: the iOS Keychain through `apple-native-keyring-store` with the `protected` feature** (the `keychain` feature is `compile_error!` on iOS). `[target.'cfg(target_os = "ios")'.dependencies] apple-native-keyring-store = { version = "1.0.2", features = ["protected"] }` and a fourth arm in `credentials.rs` (lines 275 to 281): `let store = apple_native_keyring_store::protected::Store::new();`. Pass the access-policy modifier `after-first-unlock-this-device-only` from `NativeStore::entry` so the playback grant is readable while the phone is locked during background audio (the default `WhenUnlocked` fails with -25308 exactly then). Call `credentials::keep_profile(&dirs.state)` unconditionally on iOS (the container UUID in the path can change on update, which would orphan the Keychain items). Ship an entitlements file with `keychain-access-groups = ["$(AppIdentifierPrefix)<bundle id>"]`; the sideloading signer applies it. A missing entitlement surfaces as `Error::Unavailable` (signed-out app, no panic). Optional `ios-memory-credentials` feature using `keyring_core::mock::Store` for simulator bring-up only.
7. **Sign-in: keep the PKCE loopback flow, present it in-app.** Both Spotify client IDs only have loopback `http://127.0.0.1:<port>/login` redirect URIs registered, so ASWebAuthenticationSession (https or custom scheme callback) is not an option without the maintainer changing the Spotify dashboard. Opening Safari as a separate app backgrounds Spotifast, which iOS suspends within seconds on first sign-in (no audio yet), so the listener never answers. Fix: an `SFSafariViewController` presented by the Swift side over the egui view controller keeps the process foreground; the loopback listener, PKCE and the 10-minute timeout in `auth.rs` stay unchanged. Plumbing: a C-ABI hook `spotifast_ios_open_auth_url(const char*)` implemented in Swift; the backend must also emit the **playback** flow's URL to the UI (today `LocalPlayback::Authorizing` carries none, `backend.rs:1097-1111`); `spotifast_ios_auth_finished()` fired from the `WebSignedIn / PlaybackAuthorized / SignInEnded / PlaybackAuthEnded` handlers dismisses the sheet; `safariViewControllerDidFinish` maps to `Action::CancelSignIn`. Note SFSafariViewController does not share Safari's cookies (iOS 11+), so the user signs in once; iCloud Keychain autofill works.
8. **Opening URLs.** The `open` crate compiles on iOS but spawns `uiopen`, which the sandbox forbids. `opener.rs` gets an iOS arm; URL opens go through the UI thread (`ctx.open_url`, which egui-winit routes to `webbrowser`'s UIKit backend) instead of the spawned thread at `app.rs:11518` and the `spawn_blocking` calls at `backend.rs:3144/3569`. Folder opening is hidden on iOS. `open` moves to the non-iOS table (librespot-oauth still pulls it in transitively; harmless, never called).
9. **Audio session.** The cpal fork's iOS host exposes one device ("Default Device") whose configs are derived from the shared `AVAudioSession` at call time: if the session is not configured and active it may report 0 channels and `Output::open` fails. cpal never sets the category. So `main.swift`, before `spotifast_main()`: `setCategory(.playback, mode: .default)`, `setPreferredSampleRate(44100)` (otherwise the 64-tap resampler runs at 48 kHz on most routes; acceptable but costs CPU), `setActive(true)`. `Info.plist`: `UIBackgroundModes = [audio]`. Hide the output-device picker on iOS. Headphone unplug maps to `DeviceChanged`, which fastframe-audio deliberately does not reopen on, so playback continues on the speaker; if an auto-pause is wanted the Swift side observes the route-change notification and calls a pause hook (product decision, default: follow fastframe-audio's behaviour).
10. **Now Playing and lock-screen controls: Swift side via C-ABI hooks.** fastframe-now-playing compiles on iOS but with its no-op controller. Rather than widening the fastframe crate, `src/ios/now_playing.rs` implements the same seam the app already uses (`MediaControls::start` at `app.rs:966-969`, the `Controller`/`State`/`Track`/`Command` shapes) and calls Swift through registered function pointers: `now_playing_update(title, artist, album, duration_ms, position_ms, playing, artwork_url)` and `now_playing_clear()`. Swift drives `MPNowPlayingInfoCenter` and `MPRemoteCommandCenter` and sends play/pause/next/previous/seek back through `spotifast_ios_command(cmd, value)`. This is the user-requested feature; it must not ship stubbed without saying so.
11. **Discovery is desktop-only.** `mdns-sd` compiles, but iOS 14+ requires the `com.apple.developer.networking.multicast` entitlement for an app's own multicast sockets, and a free Apple ID cannot sign it. Gate `mdns-sd`, `zeroconf.rs`, `cast.rs`, `sonos.rs` and their `backend.rs` callers behind `cfg(not(target_os = "ios"))` (or make discovery return an empty list with a log line on iOS) and hide the Cast/Sonos/LAN-receiver UI. **Consequences:** the iPhone is not discoverable on the LAN as a Spotify Connect speaker, Chromecast and Sonos targets are unavailable. Spotify Connect remote control of other devices still works because librespot-connect registers through Spotify's cloud dealer, not mDNS. A future `NWBrowser`/`NSNetServiceBrowser` bridge from Swift (exempt from the entitlement) could restore browsing.
12. **Gated out on iOS, kept for desktop.** Rule: only what does not compile leaves the iOS dependency graph (eframe's `glow`, fastframe-shell, fastframe-titlebar, `rfd`) and the TLS features are swapped; every crate that compiles for iOS stays in the graph (fastframe-update, -tray, -instance, -autostart, -now-playing, `mdns-sd`, `open`) and is made inert **at its call sites** on iOS. This keeps the `cfg` churn small. In detail: `rfd` (non-iOS table; `Command::ChoosePlaylistCover` / `ChooseLocalFilesFolder` producers at `backend.rs:694-703, 1309-1335` gated; "Local files" row and cover upload hidden), self-update (`fastframe-update` non-iOS table; `intercept`, the daily check, the Update dialog and About button gated; `check_for_updates` forced false in memory), tray, autostart, single-instance (not called from the iOS entry), the Winamp skin window and MilkDrop (`ToggleWinampWindow`/`ToggleWinampOnTop`/`SetWinampTaskbar` inert; `winamp_window = false` and `milkdrop_open = false` forced in memory without marking settings dirty; topbar buttons, shortcuts and Settings sections hidden; build with `--no-default-features`). Settings fields are **never removed**; iOS only forces values in memory.
13. **Time zones.** jiff does not bundle the tz database on iOS and reads `/usr/share/zoneinfo` plus `/etc/localtime`, whose sandbox visibility is unknown; failure degrades silently to UTC. Add `[target.'cfg(target_os = "ios")'.dependencies] jiff = { version = "0.2", features = ["tzdb-bundle-always"] }` and have `main.swift` pass `TimeZone.current.identifier` to Rust (set `TZ` before any jiff call).
14. **Interface.** A compact layout under 600 pt of content width (section 6). Safe area: one child `Ui` on `ctx.content_rect()` at the top of `ui::show` (the egui fork already plumbs `safe_area_insets` from winit into `content_rect`; toasts, popups, menus and modals already respect it; panels do not). Portrait-only for v1 (`UISupportedInterfaceOrientations` in Info.plist). Touch profile in `theme.rs`.
15. **Equaliser: the existing 10-band engine stays; bass boost is added as a low-shelf.** See `EQ_INTERFACE.md`. The user's 6-band specification applied only if no equaliser existed; a 6-band schema would break `settings.json`, saved presets and the Winamp window.
16. **Bundle identifier `com.placeholder.spotifast`** (the user replaces it), `CFBundleExecutable Spotifast`, version from `cargo metadata` filtered by package name, `MinimumOSVersion` 16.0 via `IPHONEOS_DEPLOYMENT_TARGET=16.0` (also exported before `cargo` so ring's C objects record the same target). The Keychain service string `rocks.spotifast.Spotifast` in `credentials.rs:22` is a service name, not a bundle id, and stays.
17. **Policy notes for the eventual upstream PR** (not blockers for the fork): `check-new-settings.py` fails a PR that adds `eq_bass_boost_db` until the maintainer labels it `setting-approved`; any `Cargo.lock` change breaks the Nix job until `flake.nix`'s vendor hash is regenerated on a Nix host; `AGENTS.md` forbids merge commits, so the per-agent branches are rebased or squashed onto `ios-port`, never merged with a merge commit; the fastframe/egui/winit forks may only be changed by the maintainer, so anything needing a fork change is reported, not patched.

## 3. Dependency audit by area

Verdicts: **as-is** compiles and behaves acceptably; **gate** compile but must be excluded or made inert on iOS; **replace** needs an iOS implementation; **blocker** stops the build or a required feature until fixed. Line numbers are for v0.14.1.

### 3.1 fastframe v0.14.0 (all 20 crates read)

| Crate | Verdict | Notes |
| --- | --- | --- |
| fastframe-audio | as-is | cpal iOS host via `target_vendor = "apple"`; feeder priority stub returns false on iOS (`feeder.rs:129-134`); no default-device watch on iOS; route changes arrive as cpal errors and `maintain()` reopens on all but `DeviceChanged`. Needs the AVAudioSession set up first (decision 9). |
| fastframe-shell | gate (was "blocker") | Compiles, but carries eframe with `glow` and its run loop model (run_native returns, reopen, headless) cannot exist on iOS. Excluded from the iOS graph; iOS has one `run_native`. |
| fastframe-titlebar | gate | Inert on iOS (`enabled()` false, zero footprint) but carries eframe `glow`. Excluded; call sites shimmed. |
| fastframe-now-playing | replace | Compiles with the `Unsupported` controller; `souvlaki` is not in the iOS graph. Real implementation per decision 10. |
| fastframe-update | gate | Compiles (`Platform::Other`); a sideloaded app cannot self-replace. |
| fastframe-tray | gate | `Tray::spawn` returns `None`; app already handles it. |
| fastframe-autostart | gate | Reports `Unsupported`, which already hides its row. |
| fastframe-instance | gate | Socket path likely exceeds `sun_path` (104 bytes) inside the container; runs unguarded with a warning. Not called on iOS. |
| fastframe-power | as-is | Default `Conditions`, no events; background life comes from the audio session. |
| fastframe-dirs | as-is | `directories` uses its mac module on iOS: config and state in `Library/Application Support/me.paolino.spotifast`, cache in `Library/Caches/me.paolino.spotifast`, inside the app container. Correct iOS locations. |
| fastframe-file | as-is | Atomic replace works on the container's APFS volume. |
| fastframe-fonts | replace (v1: accept) | Falls into the Linux/fontconfig arm: `fc-match` cannot spawn, no font directories exist, so no fallback faces for CJK/Arabic/Hebrew/Devanagari/Thai (tofu). The CoreText macOS arm is gated `target_os = "macos"` and would need widening to `target_vendor = "apple"` in the fastframe fork plus iOS font paths; device test needed for sandbox readability. **v1 ships Inter-only.** |
| fastframe-emoji | replace (v1: accept) | Same arm problem: no Apple Color Emoji, monochrome Noto fallback is used. Optional later: bundle Noto Color Emoji for iOS (~10 MB) or a fork iOS arm. |
| fastframe-i18n | as-is | `sys-locale` uses CoreFoundation preferred languages on all Apple targets. List languages in `CFBundleLocalizations`. |
| fastframe-log | as-is | Log and panic log land in the state folder. Consider `UIFileSharingEnabled` so testers can pull them. |
| fastframe-text | as-is | Generic default (slight hinting); optionally use the macOS convention on iOS in `theme.rs`. |
| fastframe-theme, -scroll, -icons, -desktop | as-is | Pure egui/data code. |
| fastframe-dock | n/a | macOS-only dependency; not in the iOS graph. |

### 3.2 UI stack

| Item | Verdict | Notes |
| --- | --- | --- |
| eframe (fork) `run_native` | as-is with wgpu | iOS path exists; `run_and_return` compiled out; creator closure runs in `resumed`; `storage_dir` returns `None` on iOS so pass `persistence_path`; every `ViewportBuilder` geometry is ignored (window always full screen). |
| glutin / glutin-winit via eframe `glow` | blocker (resolved by decision 1) | `compile_error!` on iOS, confirmed by the probe. |
| wgpu 30.0.1 / egui-wgpu (fork) | as-is | Metal backend on `target_vendor = "apple"`; this is the configuration upstream egui checks for `aarch64-apple-ios`. |
| winit (fork) iOS backend | as-is (unknown: fork integrity) | Present (`platform_impl/ios`), `inner_size`/`inner_position` report the safe-area rect, `run_app` calls `UIApplicationMain`. Shared-code fork patches are believed not to touch iOS; the first full build settles it. |
| egui-winit safe area | as-is | `safe_area.rs` reads `UIWindowScene.keyWindow.safeAreaInsets` into `RawInput`; `ctx.content_rect()` exposes it. Unverified: the lookup through `connectedScenes` for an app without a scene manifest; if it yields nothing, insets are zero (content under the Dynamic Island) and the fallback is an egui-fork patch reading `UIApplication.windows`. Verify on the simulator first. |
| accesskit_winit 0.32.2 | as-is | Null adapter on iOS: no VoiceOver. Documented limitation. |
| arboard 3.6.1 (clipboard) | as-is | egui-winit uses its in-process string fallback on iOS: copy/paste works inside the app, never reaches `UIPasteboard`. |
| webbrowser 1.2.4 (`links`) | as-is | Real UIKit `openURL` backend; main thread only; http(s) only. |
| rfd 0.17.2 | blocker (resolved by gating) | Confirmed by the probe. Four call sites, all in `backend.rs` (698, 702, 1313, 1329). |
| open 5.4.2 | replace | Compiles; spawns `uiopen`; fails at runtime. Decision 8. |
| Touch input | as-is | Multi-touch reaches egui; pointer emulation for the first finger; pinch/scroll from touch events; winit gesture recognizers off. |
| Software keyboard | as-is with gaps | winit's view implements `UIKeyInput`: focusing a `TextEdit` shows the keyboard, characters arrive as text events. **Gaps:** the Return key becomes `Key::Character("\n")`, which egui drops (no `Key::Enter`); no keyboard-frame notification, so egui does not know the keyboard covers the lower half; no IME composition (CJK input degraded); no secure/URL keyboard types. Fixing Return and keyboard height needs a winit-fork change (maintainer-only). **v1: accept**; keep text fields in the upper half, no Enter-only submit paths (search already runs as you type). |
| Orientation and status bar | decision | winit defaults to all orientations; eframe exposes no iOS window attributes. Portrait-only via Info.plist for v1; status-bar style via Info.plist (`UIViewControllerBasedStatusBarAppearance = NO`). |

### 3.3 librespot, networking, audio dependencies

| Item | Verdict | Notes |
| --- | --- | --- |
| librespot fork crates | as-is | Only `cfg(unix)` in `core/src/cache.rs`; the audio fetcher's tempfiles land in the container's tmp. librespot-oauth is compiled but never called. librespot-discovery is not in the tree. |
| TLS roots (`rustls-native-certs` 0.8.4) | blocker (resolved by decision 5) | Empty store on iOS; librespot aborts, reqwest fails every request. |
| hyper-proxy2 fork, hyper-util / reqwest system proxy | as-is | `system-configuration` is macOS-only, so proxy mode "System" means environment variables only (none in the sandbox) on iOS. Relabel or hide "System" on iOS; manual HTTP/SOCKS proxies still work. |
| librespot-playback decoders / backends | as-is | symphonia is pure Rust; Spotifast's own `Sink` means librespot's backends are never used. |
| rodio 0.21 (no default features) | as-is | Mixer and queue only; no cpal of its own. |
| cpal fork | as-is | Complete iOS RemoteIO host; its own CI builds `aarch64-apple-ios` and `-sim`. One device, configs derived from the live session (decision 9). Interruptions (calls, Siri) are handled: stop on Began, `setActive(true)` and resume on Ended+ShouldResume. |
| mdns-sd 0.21 | gate | Compiles; multicast entitlement unavailable to free Apple IDs (decision 11). |
| ring 0.17.14 | as-is | `ios64` assembly path for both device and simulator; needs Xcode clang via `cc`. |
| jiff 0.2.35 | replace (decision 13) | No bundled tzdb on iOS; relies on filesystem paths of unknown sandbox visibility. |
| sysinfo 0.36 / objc2-io-kit | as-is | Links IOKit on iOS; `objc2-io-kit` lists `aarch64-apple-ios`. Watch the first link step. |
| protobuf, vergen-gitcl | as-is | Host-side codegen; CI builds from a git checkout (or `VERGEN_IDEMPOTENT=1`). |
| tokio | as-is | `rt-multi-thread`, `net`, `fs`, `signal` all fine on iOS. |

### 3.4 Spotifast source tree: gating list for ios-platform

Compiles as-is on iOS (fallback arms exist): `paths.rs`, `power.rs`, `motion.rs`, `notch.rs`, `thumbbar.rs` (neutral part), `window.rs`, `autoscroll.rs`, `single_instance.rs` (not called), `http.rs`, `sink.rs`, `eq.rs`, `limiter.rs`, `vis.rs` (milkdrop off), `resample.rs`, `player.rs`, `settings.rs`, the `mac_*` modules (excluded by their own `cfg`), `appearance.rs` (Linux-only), `ui/*` (runtime `cfg!` booleans take the Linux-like branches).

Needs work (file: what):

- `Cargo.toml`: target tables per decisions 1, 2, 5, 6, 8, 11, 12, 13; `[workspace] members = ["ios"]`; comments on every new entry (AGENTS.md asks for them).
- `lib.rs`: `#[cfg(target_os = "ios")] pub mod ios;`.
- `src/ios/` (new): `mod.rs` (`spotifast_main`, hooks, eframe wrapper), `now_playing.rs`, `auth_sheet.rs` (hook plumbing), `opener` arm.
- `entrypoint.rs`: `cfg(not(target_os = "ios"))` around `eframe::glow` uses (647 to 661, 1573) and the fastframe-shell loop; otherwise unchanged (it is binary-only).
- `credentials.rs`: iOS store arm (275 to 281), access-policy modifier in `NativeStore::entry` (265 to 267), `cfg(not(windows))` at 299 and 618 narrowed so the iOS arm is explicit; `keep_profile` on iOS in `paths.rs` (64 to 80).
- `opener.rs`: iOS arm; existing arm narrowed to `not(any(windows, target_os = "ios"))`.
- `app.rs`: `Action::OpenUrl` (11518) via `ctx.open_url` on iOS; Winamp/MilkDrop actions inert on iOS (11552); `AppOptions` for iOS; `fastframe_shell`/`fastframe_titlebar` shims; the `MediaControls::start` seam (966 to 969) selecting the iOS controller; auth-sheet hooks in the sign-in handlers; `WaitingForBrowser` (2712) opening the sheet.
- `backend.rs`: `rfd` producers gated (694 to 703, 1309 to 1335); discovery callers gated (around 3882 to 3894); `sign_in_source` (3143 to 3147) and `authorize_playback_in_browser` (3568 to 3572) routed through the iOS hook; `LocalPlayback::Authorizing` carries the URL.
- `zeroconf.rs`, `cast.rs`, `sonos.rs`: module-level `cfg(not(target_os = "ios"))` or empty-result arms.
- `updates.rs`, `ui/update.rs`: early return on iOS; `check_for_updates` forced off in memory.
- `ui/settings.rs`: `.when(!cfg!(target_os = "ios"))` on rows 660, 674, 692, 706, 720; skip bodies 850 to 857, 873 to 926, 955 to 973, 994 to 1040; skins (1647 to 1830) and MilkDrop (1842 to 2010) sections gated; About update button hidden; output-device and zoom rows hidden; proxy "System" relabelled. (Shared with mobile-ui: ios-platform owns the gating of desktop-only rows, mobile-ui owns layout. Coordinate through the orchestrator if both need the same lines.)
- `ui/topbar.rs` (545 to 569) and `ui/keys.rs` (76 to 90): Winamp/MilkDrop entries skipped on iOS.
- `window.rs`: `fixed_size()` true on iOS so `keep_room_for_panels` is skipped.
- Info.plist does **not** carry `CFBundleURLTypes` (winit runs `UIApplicationMain` without a delegate, so a `spotify:` link could never reach the app, and the scheme would fight the real Spotify app) nor the local-network keys (discovery is off; loopback needs no permission).

### 3.5 Hard errors inside Spotifast itself (from source; the probe never reached the crate)

1. `credentials.rs:275-281`: no `let store` arm for iOS, so `store` is undefined (E0425). Fixed by decision 6.
2. `entrypoint.rs` `eframe::glow` references (binary target only). Fixed by decision 1 gates.

Everything else is expected to type-check; the first iOS run of the probe after ios-platform's first commit confirms it.

## 4. Audio path (for audio-dsp and the reviewer)

Per decoded packet (about 1024 to 2048 frames, interleaved stereo f64 at a fixed 44 100 Hz), on **librespot's player thread** (`PlayerInternal`, a std thread running its own tokio runtime):

1. symphonia decode.
2. If `Settings.normalisation` is on: librespot's own Dynamic limiter (log-domain, attack/release/knee) upstream of the EQ. librespot's volume multiply is a no-op because the Player is given `NoOpVolume` (`player.rs:885`).
3. `vis::Tapped::write` (`vis.rs:212-248`): processing-reset check, then `eq::Processor::process` (preamp, the solved peaking biquads, mono, balance), then `AudioTap::push` (post-EQ, pre-volume, normalisation undone for the visualisers), then `Limiter::process` with ceiling `1 / volume` (`limiter.rs`; -1 dBFS settle, 8 ms look-ahead, 100 ms release).
4. `RodioSink::write` (`sink.rs:806-903`): f64 to f32, `Resampler` if the device rate is not 44 100, `SamplesBuffer` appended to rodio's queue; blocks with `park_timeout` when more than 200 ms is queued.

On the **device callback thread** (cpal via fastframe-audio): `MixerRender::render` (`sink.rs:669-683`) pulls rodio's mixer and queue, applies the volume ramp. Nothing of Spotifast's allocates there; rodio's chain takes short std mutexes (pre-existing, out of scope).

Consequences for Phase 3:

- "The audio thread" for the EQ means librespot's player thread. The `EqSettings` mutex (`eq.rs:446`) and the `Vec` allocations in `rebuild()`/`chain()` are on that thread, not the callback. The port still replaces the mutex with a lock-free snapshot cell and makes `process` allocation-free (EQ_INTERFACE.md), and the reviewer checks that nothing new lands on `MixerRender::render` or rodio's pull path.
- The bass-boost shelf goes inside `eq::Processor::process`, after the band chain and before mono/balance, so the tap and the limiter see it. `Processor::reset` must clear its state; the `vis.rs` reset test should set `bass_boost_db = 12.0`.
- The final limiter already prevents clipping of a +12 dB shelf (sustained input settles at -1 dBFS). Its 100 ms release may pump on bass-heavy material; if audible, cap the slider at +9 dB rather than changing the limiter's thresholds.
- Persistence: flat fields in `settings.json` (`eq_on`, `eq_preamp_db`, `eq_bands_db`, `eq_user_presets`, `balance`, `mono`), rebuilt into `EqSettings` by `app::eq_settings` (`app.rs:12851`) and pushed by `App::push_eq` (`app.rs:4045`) on every change. UI: the Settings page section (`ui/settings.rs:2027-2141`, with `eq_slider`, `eq_curve`, `eq_preset_controls`) and the Winamp EQ window (`ui/winamp/equalizer.rs`, whose graph is a spline through the slider positions, not the real response). Presets: `eq::PRESETS` (Winamp's 18, then the app's own), identity by exact `bands_db` equality.
- iOS specifics: the cpal iOS host offers only the session's current sample rate, so without `setPreferredSampleRate(44100)` the resampler runs (48 kHz on most routes). `raise_feeder_priority` is a no-op on iOS.

## 5. Sign-in (for ios-platform)

- Three grants: shared Web API client `d420a117a32841c2b3474932e49fb54b` (redirect `http://127.0.0.1:8989/login`), optional personal Web API client, playback client `65b708073fc0480ea92a077233ca87bd` (Spotify's desktop client identity, redirect `http://127.0.0.1:8898/login`). Authorization Code + PKCE in `src/auth.rs`: `listen_for_redirect` binds a std `TcpListener` on 127.0.0.1, `wait_for_code_on_listener` selects over accept / cancel / 10-minute deadline, answers an HTML page that calls `window.close()`. Token exchange via the shared reqwest client. librespot-oauth is not used.
- Playback: Spotifast builds `AUTHENTICATION_SPOTIFY_TOKEN` credentials from the access token; librespot exchanges them over the AP connection for a reusable stored credential that `credentials::Store` persists on the credential worker thread.
- iOS plan: decision 7 (SFSafariViewController plus unchanged loopback listener), decision 5 (TLS roots, otherwise the token exchange fails), decision 6 (Keychain).
- **Unknowns only a device can answer:** whether `accounts.spotify.com` accepts the desktop playback client id from Mobile Safari, and whether Spotify's access points accept the token and the stored credential from a librespot client that announces `OS_IPHONE / PLATFORM_IPHONE_ARM64` (librespot derives it from `std::env::consts::OS`; the fork already spoofs Android as Linux for a similar rejection at `handshake.rs:123-128`). If either is refused, **report it as a blocker**; the fix would be in the librespot fork, which only the maintainer may change.
- Loopback binding needs no entitlement and no Local Network prompt. Cancel an in-flight flow on `didEnterBackground` so a suspended listener does not hold the port.

## 6. Interface (for mobile-ui)

Today: 88 pt bottom player bar, 210 to 600 pt left sidebar, up to three right panels of at least 280 pt, a central page whose top bar needs about 474 pt, a 760 by 520 window floor, a 440 pt sign-in card, 420 pt modals, 320 pt devices popup. An iPhone is 375 to 430 pt wide in portrait. Already adaptive and reusable: compact `TrackRow` with width-driven column dropping (works at 390 pt), home tiles and card grids wrapping to two columns, page heroes shrinking below 720, settings rows stacking below about 400 pt, panel widths clamped with `ctx.content_rect()` in `update.rs:41` and `sidebar.rs:397`.

Work list (files are mobile-ui's allowlist; anything else is reported):

1. **Breakpoint:** `theme::compact(ctx)` = `ctx.content_rect().width() < 600.0` (or iOS). Switch point `ui/mod.rs::show` and `::central` (72 to 85): hide the sidebar panel (topbar already shows PanelLeft/Home when `sidebar_visible` is false), right panels become full-screen overlays, two-row top bar variant, `player_bar::show` region split (95 to 125) becomes a two-row or mini bar expanding to a now-playing overlay. Sign-in card `min(440, width - 32)` with 20 pt margins, anchored near the top; modals and the devices popup `min(x, content_rect.width() - 16)`.
2. **Safe area:** at the top of `ui::show`, paint `palette.panel` over `ctx.viewport_rect()` and run everything inside `ui.new_child(UiBuilder::new().max_rect(ctx.content_rect()))`. Desktop is unchanged (insets are zero). Do not add insets to toasts, popups, menus or modals (already constrained; would double-pad). Generalise `theme::titlebar_inset` (246) into a `top_inset` if any per-site padding remains.
3. **Touch profile** in `theme.rs` (386 to 390) under compact/touch: `interact_size (44, 44)`, `button_padding (16, 12)`, `menu_margin 8`, `ICON_BUTTON_PAD 12 → 26`, menu row height 28 → 44 (`widgets.rs:449/555`), slider hit height 16 → 44 with the bar still 4 pt (`widgets.rs:2993`), switch 40×22 → 51×31, `nav_button` 32 → 44 (`topbar.rs:164`), transport slots 36 → 44, `eq_slider` 30 → 44 wide.
4. **Touch behaviour:** single tap on a row body plays in compact mode (`widgets.rs:2053-2092`; multi-select behind long press); rows allocated with `Sense::click()` and no `DragAndDrop::set_payload` in compact mode (`widgets.rs:1490/1513`, `sidebar.rs:242`, `player_bar.rs:569`) so swipes scroll instead of picking rows up; More buttons, card play discs and slider handles always visible (`widgets.rs:2023/2704/3080`, `sidebar.rs:93/183`); context menus already open on a 0.8 s long press in this egui fork (no change; optionally `max_click_duration` 0.5 s); lyrics side panel's manual-scroll detection should also look at pointer drags (`lyrics.rs:306-309`, mirroring 737 to 741); check the devices popup closes on an outside tap (`devices.rs:380-396`), add `clicked_elsewhere()` if not.
5. **Keyboard:** no Enter-only submit paths on iOS; text fields in the upper half or inside a `ScrollArea`; shortcut hints hidden on iOS (`keys.rs:11`, `topbar.rs:243/547/562`).
6. **Equaliser screen:** `Page::Equalizer` (`model.rs`, dispatch in `ui/mod.rs:317-333`, a button in the player bar extras or top bar), reusing `widgets::switch`, `widgets::chips`, `eq_curve`, `eq_preset_controls`, `eq_slider` in a `horizontal_wrapped` grid with at least 44 pt columns (all ten bands, two rows of five), plus the Bass boost slider below (EQ_INTERFACE.md). The Settings section gets the same Bass boost slider. The Settings EQ row currently needs 470 pt unwrapped and must wrap under the breakpoint.
7. **Tests:** `cargo run --features demo -- --demo --demo-size 390x844 --demo-shot phone.png` already renders a phone-size frame; add `frame_sized(ctx, app, size, insets)` beside `demo.rs:6500` and `every_surface_renders_on_a_phone` cloned from `demo.rs:7546` with `screen_rect` 390×844 and `safe_area_insets: Some(SafeAreaInsets(MarginF32 { top: 59.0, left: 0.0, right: 0.0, bottom: 34.0 }))`, asserting nothing lies outside `content_rect`.

## 7. Build and packaging (for packaging)

- `build.rs` is host-only: cross-compiling from macOS runs only the catalog compilation. `panic = "abort"` is right for a staticlib (unwinding into Swift is UB); the panic hook still writes the panic log first. Keep an unstripped `libspotifast.a`/`.app` for symbolication (`STRIP_INSTALLED_PRODUCT=NO`).
- Rust build: `cargo build --release --target aarch64-apple-ios -p spotifast-ios` (and `aarch64-apple-ios-sim` for the simulator gate) with `IPHONEOS_DEPLOYMENT_TARGET=16.0` exported. Never `--features demo`.
- Xcode project: a hand-written minimal `ios/Spotifast.xcodeproj/project.pbxproj` with one application target, `ios/Sources/main.swift` (top-level code, no `@main`), `INFOPLIST_FILE = ios/Info.plist`, `IPHONEOS_DEPLOYMENT_TARGET = 16.0`, `LIBRARY_SEARCH_PATHS` into `target/aarch64-apple-ios[-sim]/release`, `OTHER_LDFLAGS` linking `libspotifast_ios.a` plus the frameworks (UIKit, Foundation, CoreFoundation, Security, AVFoundation, AudioToolbox, CoreAudio, MediaPlayer, SafariServices, Metal, QuartzCore, IOKit, CoreGraphics, CoreText; trim after the first link). `xcodebuild build -sdk iphoneos CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO` (not `archive`/`-exportArchive`, which want signing), then `mkdir Payload && cp -R Spotifast.app Payload/ && zip -r dist/Spotifast-<version>-ios-unsigned.ipa Payload`. Simulator gate: `-destination 'generic/platform=iOS Simulator'` against the `-sim` library.
- Info.plist (hand-written, not the macOS one): `CFBundleIdentifier com.placeholder.spotifast`, `CFBundleExecutable Spotifast`, `CFBundleShortVersionString` from `cargo metadata` filtered by name, `UIBackgroundModes [audio]`, `UILaunchScreen {}` (needed for full-screen layout on modern devices), `UISupportedInterfaceOrientations` portrait, `UIViewControllerBasedStatusBarAppearance NO`, `CFBundleLocalizations`. No URL types, no local-network keys (section 3.4).
- Entitlements file with `keychain-access-groups`; kept in the unsigned `.ipa` for the signer.
- Icons: the only 1024 px raster (`packaging/macos/icon-1024.png`) is a macOS disc with about 40 % transparent margin, unusable as an iOS icon (iOS composites an opaque square under its own mask; `actool` warns on alpha). Generate a new fully opaque 1024 PNG from `packaging/icons/spotifast.svg` at about 85 % scale over the SVG's rim colour (#18c055 / #0c8a3a), verify every pixel has alpha 255, place it in `ios/Assets.xcassets/AppIcon.appiconset/` with a single-size `Contents.json` (Xcode 14+). Do not touch `packaging/macos` or `packaging/icons`; do not name files review/compare/before/after (CI rule).
- GitHub Actions: `runs-on: macos-latest` (currently macOS 26 with Xcode 26.x), `dtolnay/rust-toolchain@stable` then `rustup target add aarch64-apple-ios aarch64-apple-ios-sim` **after checkout** (the pinned toolchain comes from `rust-toolchain.toml`; the first probe failed because the targets were added to the wrong toolchain), export `IPHONEOS_DEPLOYMENT_TARGET=16.0`, run `scripts/build-ipa.sh`, `upload-artifact` with `if-no-files-found: error`.
- Desktop CI hazards: `ci.yml`'s `packages[0]` (decision 3); `tests/branding.rs` walks every new directory and reads every file (keep output under `dist/` and `target/`, never commit an `.ipa`); `packaging_macos.rs` and `packaging_icons.rs` break if the macOS bundle script or the SVG change (do not change them).

## 8. What cannot work on a sideloaded iPhone build (said plainly)

- **Winamp skin window, MilkDrop, tray and "close keeps playing" background-window mode.** One full-screen window, `run_native` never returns, no child processes.
- **Self-update.** A sideloaded app cannot replace itself; AltStore/SideStore handle re-signing.
- **LAN discovery: being discoverable as a Spotify Connect speaker, Chromecast, Sonos.** Needs the multicast entitlement a free Apple ID cannot grant. Connect control of other devices still works through the cloud dealer.
- **Native file pickers** (local-files folder, playlist-cover upload) in this phase. A `UIDocumentPickerViewController`/`PHPicker` bridge is possible later.
- **System proxy settings**, Start at login, taskbar/dock integration, keyboard shortcuts without a hardware keyboard, VoiceOver (accesskit has no iOS adapter), system clipboard, colour emoji and non-Latin fallback fonts (v1), the Return key on the software keyboard, IME composition.
- **Possibly: sign-in itself**, if Spotify refuses the desktop playback client from Mobile Safari or the AP refuses an `OS_IPHONE` librespot client. Only a device test tells. Everything in the plan is arranged so this is the first thing tested.
- **7-day re-sign limit** on free Apple IDs applies to any sideloaded app (AltStore/SideStore refresh it).

## 9. Working constraints for Phases 2 to 4

- **No macOS here.** Every iOS compile check, simulator build and `.ipa` goes through the fork's Actions on `ltaIian/spotifast` (`ios-port` is the fork's default branch so `workflow_dispatch` works; push the agent branch and dispatch `ios-probe.yml` or the packaging workflow with `--ref`). An agent never claims an iOS build works without a run that shows it.
- **No Rust locally yet.** Desktop `cargo build`/`cargo test` needs rustup plus the MSVC C++ build tools (Visual Studio 2022 Build Tools is installed but without the C++ workload). Only about 41 GB of disk is free; a Spotifast debug target directory is several GB and each agent worktree gets its own (`AGENTS.md` forbids sharing one). Decision needed from the user before Phase 2: install Rust and the C++ workload locally (about 8 GB plus targets), or run desktop checks on CI only (`ci.yml` has `workflow_dispatch`; slower feedback for agents).
- `--no-default-features` for every iOS build (MilkDrop needs CMake, libprojectM and its own winit loop).
- Policy: dependencies only from crates.io or `github.com/crmne`; no fork changes by us; report what needs one (winit Return key and keyboard height, fastframe-fonts/-emoji Apple arms, fastframe-audio QoS arm, egui-winit safe-area fallback and `UIPasteboard`, librespot `platform_for` if the AP rejects iOS).
- Git: one topic per commit, each compiling; agent branches are rebased or squashed onto `ios-port`; no merge commits.

## 10. Device test list (needs a real iPhone; not provable in CI)

1. TLS round trip: sign-in page loads in the in-app sheet, the token exchange succeeds, librespot's apresolve succeeds (a failure aborts the process, so check `panic.log`).
2. Spotify accepts the desktop playback client id from the in-app Safari sheet; the AP accepts `OS_IPHONE`; a stored playback credential is persisted and restored after relaunch; also after a relaunch with the device locked (access policy).
3. Keychain: no `-34018` ("platform failure" in the log) after re-signing with the chosen sideloader.
4. Background audio with the screen locked; lock-screen title/artwork and play/pause/next/previous/seek from the lock screen, Control Center and headphone buttons.
5. Phone-call interruption and resume; headphone unplug behaviour; Bluetooth route switch; AirPlay.
6. Safe-area insets non-zero (content not under the Dynamic Island or the home indicator); portrait layout on a 375 pt and a 430 pt phone.
7. Software keyboard: typing in search; fields not hidden behind the keyboard.
8. Interface language follows the device; dates show in local time (jiff tz hand-off).
9. Emoji and non-Latin track names (expected: monochrome emoji, tofu for CJK in v1).
10. Equaliser and bass boost audible and persisted across launches.

## 11. Open decisions for the user (defaults in bold)

1. Desktop toolchain locally (**install Rust + MSVC C++ workload**) or CI-only desktop checks.
2. **Portrait-only** v1, or landscape too.
3. Headphone unplug: **keep playing on the speaker** (fastframe-audio's behaviour) or auto-pause (Apple's guideline; needs a Swift route-change observer).
4. **Ten-band equaliser kept** plus bass boost (EQ_INTERFACE.md), versus a 6-band migration (breaks saved settings and the Winamp window; not recommended).
5. Bundle id **`com.placeholder.spotifast`** as asked, to be replaced by the user.

## 12. Execution rules for Phases 2 to 4 (orchestrator, binding for every agent)

### 12.1 Scope changes from the user

- **No Windows builds.** The user excluded them on 2026-10-23. Desktop regression checking is the macOS `desktop` job of `.github/workflows/ios-probe.yml` (build without default features, tests and clippy with the `demo` feature, formatting, policy scripts) plus, on the integration branch only, the `full` job (default and all features on macOS and Linux). Nobody installs a Rust toolchain on the Windows machine.
- **The deliverable is the unsigned `.ipa`**, built by `scripts/build-ipa.sh` on the macOS runner and downloaded into `dist/`.
- Defaults taken for section 11: portrait-only; keep playing on the speaker when headphones are unplugged (fastframe-audio's behaviour); ten-band equaliser plus bass boost; bundle id `com.placeholder.spotifast`.

### 12.2 Branches, worktrees, commits

- Integration branch `ios-port`. Agent branches `ios-port-<agent>` in worktrees `C:\Users\ASUS\Desktop\Projects\spotifast-wt\<agent>`. (Git cannot hold both `ios-port` and `ios-port/<agent>`: a branch name cannot be a ref and a directory of refs at once, hence the dash.)
- An agent works **only** inside its own worktree, with absolute paths under it, and pushes **only** its own branch to the remote `fork` (`ltaIian/spotifast`). It never pushes `ios-port`, never touches the main checkout at `C:\Users\ASUS\Desktop\Projects\Spotifast`, never rewrites published history.
- Commit identity is configured in the repository (`ltaIian`). One topic per commit, clear messages, and every commit message ends with:

      Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>

- Merge order into `ios-port`: audio-dsp, mobile-ui, ios-platform, packaging. The orchestrator rebases each branch onto `ios-port` and fast-forwards; no merge commits.

### 12.3 The CI loop (there is no local compiler)

`bash /c/Users/ASUS/Desktop/Projects/spotifast-wt/ci.sh` from inside a worktree:

1. commit, then `ci.sh push` (pushes the branch, prints the run id of HEAD's checks),
2. `ci.sh wait <run>` (blocks up to 9 minutes; repeat until it says finished, or run it in the background),
3. `ci.sh fetch <run>` (downloads every job's `out/` into `spotifast-wt/ci-out/<branch>/<sha>/` and prints the error lines).

Jobs per branch: `desktop` always; `ios` on `ios-port`, `ios-port-ios-platform`, `ios-port-packaging` (or `ci.sh dispatch ios=true`); `shots` on `ios-port` and `ios-port-mobile-ui` (or `ci.sh dispatch shots=true`); `full` on `ios-port`. A newer push to an agent branch cancels its older run.

- **Cargo.lock:** CI does not build `--locked`. A branch that changes dependencies takes `out/Cargo.lock` from the `desktop` artifact of its run and commits it in the same topic (the lock file is target-independent, so the macOS resolution is complete). Read the diff before committing it: only the expected packages may change.
- **Never claim** a build, test or iOS behaviour that no run showed. Quote the run id in reports.
- The `ios` job, once `ios/` exists: `cargo check --lib` for both targets, `cargo build -p spotifast-ios` (dev profile) for the simulator, `xcodebuild` of the simulator app, then a **smoke test** that installs it in an iPhone simulator, launches it with `SPOTIFAST_SELFTEST=1` in its environment, takes screenshots at 12 s and 40 s (`out/sim-12s.png`, `out/sim-40s.png`), and collects the console, the system log, the app's own log files from its container and any crash report. On the device target, once `scripts/build-ipa.sh` exists, the release `.ipa` and an inspection of it.
- The `shots` job builds the desktop demo and takes one PNG per line of `.github/ios-port-shots.txt` (`name|demo arguments`, e.g. `eq|--demo-size 390x844 --demo-page equalizer`), falling back to a built-in list.

### 12.4 Contracts between agents

- **Xcode project and scripts:** `ios/Spotifast.xcodeproj` has one application target and a **shared scheme** named `Spotifast` (`xcshareddata/xcschemes/Spotifast.xcscheme`), product `Spotifast.app`. The project finds the Rust library through the build setting `SPOTIFAST_RUST_LIB_DIR` (absolute directory holding `libspotifast_ios.a`), passed on the `xcodebuild` command line; the project's default points at `$(PROJECT_DIR)/../target/aarch64-apple-ios/release`. CI passes the simulator debug directory; `build-ipa.sh` passes the device release directory.
- **Rust and Swift:** the C ABI lives in the `spotifast-ios` crate (`ios/src/lib.rs`), which forwards to `spotifast::ios`. Swift provides the functions Rust calls with `@_cdecl`; Rust declares them in `extern "C"` blocks under `cfg(target_os = "ios")` only, so desktop builds never see them. No Rust function called from Swift may panic across the boundary (`panic = "abort"` in release).
- **Equaliser:** `EQ_INTERFACE.md`. The declarations are already on `ios-port` (12.6); behaviour belongs to audio-dsp, interface to mobile-ui.
- **Titlebar shim:** fastframe-titlebar leaves the iOS graph, so ios-platform adds `src/titlebar.rs` (re-exporting `fastframe_titlebar` on desktop, inert stubs with the same signatures on iOS) and may make the **mechanical** rename `fastframe_titlebar::` to `crate::titlebar::` in any file, including `src/ui/`. That rename is the only edit ios-platform makes under `src/ui/`. mobile-ui adds no new `fastframe_titlebar::` uses.

### 12.5 File ownership

| Area | Owner |
| --- | --- |
| `Cargo.toml`, `Cargo.lock`, `.github/workflows/ci.yml` (the `packages[0]` fix), `ios/**` except the icon catalog, `src/lib.rs`, `src/ios/**`, `src/titlebar.rs`, `src/entrypoint.rs`, `src/credentials.rs`, `src/auth.rs` (only where iOS needs it), `src/opener.rs`, `src/backend.rs`, `src/paths.rs`, `src/window.rs`, `src/updates.rs`, `src/zeroconf.rs`, `src/cast.rs`, `src/sonos.rs`, `src/single_instance.rs`, `src/winamp.rs`, the iOS behaviour gates in `src/app.rs` | ios-platform |
| `src/eq.rs`, `src/limiter.rs`, `src/vis.rs`, the processor wiring in `src/sink.rs` and `src/player.rs`, the `SharedEq` call sites wherever they are (mechanical), `App::push_eq` and the equaliser tests in `src/app.rs`, the bass-boost lines of `src/settings.rs` | audio-dsp |
| `src/ui/**` (except the mechanical titlebar rename), `src/theme.rs`, `src/model.rs` (new `Page`/`Action` variants), `src/demo.rs` (demo pages and the phone render test), the UI action handlers it adds in `src/app.rs`, `.github/ios-port-shots.txt`, hiding desktop-only rows, buttons and shortcuts on iOS | mobile-ui |
| `scripts/build-ipa.sh`, `.github/workflows/ios-ipa.yml`, `ios/Assets.xcassets/**`, the bundle id and version keys of `ios/Info.plist`, `.gitignore` (`dist/`) | packaging |
| `docs/ios-port/**`, `.github/workflows/ios-probe.yml`, `README.md`, merges | orchestrator |

A change outside one's area is reported to the orchestrator, not made. `src/app.rs` is shared: each agent edits only the hunks its row names, so merges stay mechanical.

### 12.6 Equaliser declarations already on `ios-port`

The orchestrator committed the interface of `EQ_INTERFACE.md` as declarations only: `eq::BASS_BOOST_HZ`, `eq::BASS_BOOST_MAX_DB`, `EqSettings::bass_boost_db` (default 0, clamped to 0..12 by `clamped()`, **not yet used by the processor or the curve**), `Settings::eq_bass_boost_db` (default 0, documented in `docs/_reference/settings-and-files.md`), `app::eq_settings` mapping it, and `Action::SetEqBassBoost(f32)` with its handler (clamp, then `push_eq`). audio-dsp makes the processor and `curve()` honour it; mobile-ui draws the slider and emits the action.

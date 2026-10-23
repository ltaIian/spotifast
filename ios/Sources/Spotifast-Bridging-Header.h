// The C functions of the spotifast-ios static library (ios/src/lib.rs),
// for the Swift code.

#ifndef SPOTIFAST_BRIDGING_HEADER_H
#define SPOTIFAST_BRIDGING_HEADER_H

#include <stdint.h>

/// Runs Spotifast: UIKit's main loop, with the app in its window. Call it
/// once, on the main thread, once the audio session is set up.
void spotifast_main(void) __attribute__((noreturn));

/// Saves what quitting would save: call it when the app enters the
/// background, where iOS may suspend it and end it without warning.
void spotifast_will_background(void);

/// The app is back in the foreground: call it when it will enter the
/// foreground.
void spotifast_will_foreground(void);

/// A logic pass when none ran lately: call it once a second, on the main
/// thread, while the app is in the background.
void spotifast_background_tick(void);

/// A remote control was used, with one of NowPlaying.swift's codes: call it
/// on the main thread.
void spotifast_ios_command(int32_t command, double value);

/// Saves what quitting would save: call it when the app is about to
/// terminate.
void spotifast_will_terminate(void);

/// The person closed the sign-in sheet: the sign-in it showed is given up.
/// Call it on the main thread.
void spotifast_auth_sheet_closed(void);

#endif

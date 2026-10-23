// The C functions of the spotifast-ios static library (ios/src/lib.rs),
// for main.swift.

#ifndef SPOTIFAST_BRIDGING_HEADER_H
#define SPOTIFAST_BRIDGING_HEADER_H

/// Runs Spotifast: UIKit's main loop, with the app in its window. Call it
/// once, on the main thread, once the audio session is set up.
void spotifast_main(void) __attribute__((noreturn));

/// Saves what quitting would save: call it when the app enters the
/// background, where iOS may suspend it and end it without warning.
void spotifast_will_background(void);

/// Saves what quitting would save: call it when the app is about to
/// terminate.
void spotifast_will_terminate(void);

#endif

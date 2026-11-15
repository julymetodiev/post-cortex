// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The system's proxy settings, read once at start (owner 2026-11-01): the
//! first HTTP client of a process reads them, 3.8 s on macOS
//! (SystemConfiguration), and that was the first call of whichever module
//! calls out first (a provider, http, observe, ops), past its deadline under
//! load. Elsewhere only the environment is read, at no cost.

/// Read them now, in a thread of its own, so the daemon starts at once.
pub fn warm() {
    std::thread::spawn(|| {
        if let Err(e) = reqwest::Client::builder().build() {
            tracing::warn!("the system's proxy settings were not read: {e}");
        }
    });
}

// SPDX-License-Identifier: MIT

//! Haptic feedback. Every call is a no-op if the haptics server is unavailable (e.g. simulator).

use std::cell::OnceCell;
use std::time::Duration;

use ::haptics::HapticPattern;

haptics::use_api!();

thread_local! {
    static API: OnceCell<Option<HapticsApi>> = const { OnceCell::new() };
}

fn vibrate(p: HapticPattern) {
    API.with(|c| {
        let api = c.get_or_init(|| {
            let a = HapticsApi::try_new_with_timeout(Duration::from_millis(200));
            if a.is_none() {
                log::info!("health haptics unavailable");
            }
            a
        });
        if let Some(a) = api {
            a.vibrate(p);
        }
    });
}

/// Every 8 credited bits.
pub fn tick() {
    vibrate(HapticPattern::SharpTickOne100);
}
/// Once at >= 90% full.
pub fn ramp() {
    vibrate(HapticPattern::TransitionRampUpShortSmoothOne0to100);
}
/// Pool full.
pub fn full() {
    vibrate(HapticPattern::StrongClick100);
}
/// A source flipped to unhealthy.
pub fn alert() {
    vibrate(HapticPattern::Alert750ms);
}

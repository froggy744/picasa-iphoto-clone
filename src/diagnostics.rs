//! Opt-in aggregate performance diagnostics.
//!
//! Normal application runs do not record or print anything. Enable with
//! `PICASA_PROFILE=1` when investigating gallery performance.

use std::cell::RefCell;
use std::time::{Duration, Instant};

pub fn enabled() -> bool {
    std::env::var_os("PICASA_PROFILE").is_some()
}

/// Resident set size in MiB (Linux). Used to profile the 66k-photo model.
pub fn rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|contents| {
            contents
                .split_whitespace()
                .nth(1)
                .and_then(|resident| resident.parse::<u64>().ok())
        })
        .map(|pages| pages.saturating_mul(4096) / (1024 * 1024))
        .unwrap_or(0)
}

pub fn refresh_started(photo_count: usize) -> Option<Instant> {
    enabled().then(|| {
        eprintln!("PROFILE refresh_start photos={photo_count}");
        Instant::now()
    })
}

pub fn refresh_first_batch(started: Option<Instant>, visible_count: usize) {
    if let Some(started) = started {
        eprintln!(
            "PROFILE refresh_first_batch photos={visible_count} elapsed_ms={}",
            started.elapsed().as_millis()
        );
    }
}

pub fn refresh_finished(started: Option<Instant>, photo_count: usize) {
    if let Some(started) = started {
        eprintln!(
            "PROFILE refresh_finished photos={photo_count} elapsed_ms={} rss_mb={}",
            started.elapsed().as_millis(),
            rss_mb()
        );
    }
}

#[derive(Default)]
struct ScrollStats {
    started: Option<Instant>,
    last_frame: Option<Instant>,
    frames: u32,
    worst_frame_ms: u128,
    thumbnail_hits: u32,
    thumbnail_misses: u32,
}

thread_local! {
    static SCROLL_STATS: RefCell<ScrollStats> = RefCell::new(ScrollStats::default());
}

pub fn scroll_tick() {
    if !enabled() {
        return;
    }

    let now = Instant::now();
    SCROLL_STATS.with(|stats| {
        let mut stats = stats.borrow_mut();
        let started = *stats.started.get_or_insert(now);
        if let Some(previous) = stats.last_frame {
            let frame_ms = now.duration_since(previous).as_millis();
            stats.worst_frame_ms = stats.worst_frame_ms.max(frame_ms);
        }
        stats.last_frame = Some(now);
        stats.frames = stats.frames.saturating_add(1);

        if now.duration_since(started) >= Duration::from_secs(1) {
            eprintln!(
                "PROFILE scroll fps={} worst_frame_ms={} thumbnail_hits={} thumbnail_misses={}",
                stats.frames, stats.worst_frame_ms, stats.thumbnail_hits, stats.thumbnail_misses
            );
            *stats = ScrollStats {
                started: Some(now),
                ..ScrollStats::default()
            };
        }
    });
}

pub fn visible_thumbnail(hit: bool) {
    if !enabled() {
        return;
    }
    SCROLL_STATS.with(|stats| {
        let mut stats = stats.borrow_mut();
        if hit {
            stats.thumbnail_hits = stats.thumbnail_hits.saturating_add(1);
        } else {
            stats.thumbnail_misses = stats.thumbnail_misses.saturating_add(1);
        }
    });
}

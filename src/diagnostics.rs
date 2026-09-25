/// Resident set size in MiB on Linux; returns 0 on unsupported platforms.
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

/// Environment flags are read once per process: the trace checks run on
/// per-tile hot paths (binds, paintable assigns, cache lookups), and a
/// `var_os` lookup on every one of hundreds of thousands of calls measurably
/// slows traced sessions on its own.
pub fn trace_enabled() -> bool {
    static TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TRACE.get_or_init(|| std::env::var_os("PICASA_TRACE").is_some())
}

/// Per-tile trace lines (gtk_bind, paintable_assign, cache_lookup, queue
/// waits) fire hundreds of times per second while scrolling and dominate
/// traced logs. They are opt-in on top of `PICASA_TRACE`.
pub fn tile_trace_enabled() -> bool {
    static TILE_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TILE_TRACE.get_or_init(|| std::env::var_os("PICASA_TRACE_TILES").is_some())
}

/// Milliseconds since process start. Navigation tracing stamps every pipeline
/// stage with this so a single run's log reconstructs the click-to-frame
/// timeline without cross-referencing wall-clock clocks.
pub fn t_ms() -> u128 {
    static PROCESS_START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    PROCESS_START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis()
}

/// True once, for the first user navigation of the process. Startup model
/// batches and scan refreshes must not consume it: only sidebar/navigation
/// entry points claim it, which keeps "first click after startup" measurable
/// separately from every later navigation.
pub fn claim_first_nav() -> bool {
    static FIRST_NAV: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
    FIRST_NAV.swap(false, std::sync::atomic::Ordering::Relaxed)
}

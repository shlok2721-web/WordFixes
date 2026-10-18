fn main() {
    let build_label = std::env::var("NOTES_BUILD_LABEL")
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=NOTES_BUILD_LABEL={build_label}");
    println!("cargo:rerun-if-env-changed=NOTES_BUILD_LABEL");

    // A per-build stamp shown in the About dialog, so "which build am I
    // actually running?" is answerable at a glance. Formatted from the epoch
    // by hand: build scripts should not pull a chrono dependency for this.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let (h, m) = ((secs % 86_400) / 3600, (secs % 3600) / 60);
    // Civil date from days-since-epoch (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mth <= 2 { y + 1 } else { y };
    println!("cargo:rustc-env=NOTES_BUILD_STAMP={y:04}-{mth:02}-{d:02} {h:02}:{m:02} UTC");
    println!("cargo:rerun-if-changed=src/main.rs");
}

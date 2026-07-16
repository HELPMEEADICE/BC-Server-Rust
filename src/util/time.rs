/// Milliseconds since Unix epoch (matches Node `Date.getTime()`).
pub fn common_time() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

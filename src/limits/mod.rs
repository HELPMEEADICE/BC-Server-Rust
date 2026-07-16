use std::collections::VecDeque;

use crate::state::World;
use crate::util::common_time;

/// Returns true if connection should be rejected.
pub fn check_ip_connection_limit(
    world: &mut World,
    ip: &str,
    connection_limit: usize,
    rate_limit: usize,
) -> bool {
    let now = common_time();
    let bucket = world.ip_connections.entry(ip.to_string()).or_default();

    // Rate: last `rate_limit` connections within 1 second
    let over_rate = bucket.len() >= rate_limit
        && bucket
            .get(bucket.len().saturating_sub(rate_limit))
            .is_some_and(|&t| now - t <= 1000);

    let over_concurrency = bucket.len() >= connection_limit;

    if over_concurrency || over_rate {
        return true;
    }

    bucket.push_back(now);
    false
}

pub fn on_ip_disconnect(world: &mut World, ip: &str) {
    if let Some(bucket) = world.ip_connections.get_mut(ip) {
        if bucket.len() <= 1 {
            world.ip_connections.remove(ip);
        } else {
            bucket.pop_front();
        }
    }
}

/// Sliding window message rate limiter (CLIENT_MESSAGE_RATE_LIMIT msgs / second).
#[derive(Debug, Clone)]
pub struct MessageRateLimiter {
    bucket: VecDeque<i64>,
    limit: usize,
}

impl MessageRateLimiter {
    pub fn new(limit: usize) -> Self {
        let mut bucket = VecDeque::with_capacity(limit);
        for _ in 0..limit {
            bucket.push_back(0);
        }
        Self { bucket, limit }
    }

    /// Returns true if over limit (should disconnect).
    pub fn check(&mut self) -> bool {
        let now = common_time();
        let last = self.bucket.pop_front().unwrap_or(0);
        self.bucket.push_back(now);
        now - last <= 1000 && self.limit > 0
    }
}

/// Extract client IP from socket headers (last hop of x-forwarded-for).
pub fn extract_ip(remote: Option<&str>, x_forwarded_for: Option<&str>) -> String {
    if let Some(xff) = x_forwarded_for {
        if let Some(last) = xff.split(',').last() {
            let trimmed = last.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    remote.unwrap_or("unknown").to_string()
}

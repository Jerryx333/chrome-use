//! The daemon's bounded log of tracked requests (`network requests`,
//! `request_detail`, `expect request`, `--observe`).
//!
//! Tracking is on for the daemon's whole life whenever its stream server
//! started (`DaemonState::new_with_stream`), so this log has to be bounded:
//! it keeps the newest [`MAX_TRACKED_REQUESTS`] entries and caps what one entry
//! may hold. Positions are absolute sequence numbers, so a mark taken before
//! old entries were evicted still names the same point in the stream.

use std::collections::VecDeque;

use serde_json::Value;

use super::actions::TrackedRequest;

/// Entries kept; the oldest is evicted past this.
pub const MAX_TRACKED_REQUESTS: usize = 1000;
/// Bytes of `postData` kept per entry.
pub const MAX_POST_DATA_BYTES: usize = 64 * 1024;
/// Bytes of names plus values kept per header object (request or response).
pub const MAX_HEADER_BYTES: usize = 16 * 1024;

/// Prefix of the `request_detail` error for an id that was evicted.
pub const REQUEST_EVICTED: &str = "request_evicted:";

#[derive(Default)]
pub struct TrackedRequests {
    entries: VecDeque<TrackedRequest>,
    /// Sequence number of `entries[0]`.
    start: u64,
    /// Sequence number right after the last `clear`. Everything in
    /// `cleared_through..start` was evicted, not cleared.
    cleared_through: u64,
    /// Ids of the most recently evicted entries, so `request_detail` can tell
    /// "evicted" from "never seen". Bounded like the entries.
    evicted_ids: VecDeque<String>,
}

impl TrackedRequests {
    /// Record a request, capping its post data and headers, and evict the
    /// oldest entry when full.
    pub fn push(&mut self, mut request: TrackedRequest) {
        cap_entry(&mut request);
        if self.entries.len() >= MAX_TRACKED_REQUESTS {
            if let Some(old) = self.entries.pop_front() {
                self.start += 1;
                if self.evicted_ids.len() >= MAX_TRACKED_REQUESTS {
                    self.evicted_ids.pop_front();
                }
                self.evicted_ids.push_back(old.request_id);
            }
        }
        self.entries.push_back(request);
    }

    /// Drop every entry. The sequence keeps counting, so earlier marks stay
    /// valid; cleared entries are not reported as evicted.
    pub fn clear(&mut self) {
        self.start += self.entries.len() as u64;
        self.cleared_through = self.start;
        self.entries.clear();
        self.evicted_ids.clear();
    }

    /// The position right after the newest entry.
    pub fn mark(&self) -> u64 {
        self.start + self.entries.len() as u64
    }

    /// The entries recorded after `mark`, and how many of those were evicted
    /// before they could be read.
    pub fn since(&self, mark: u64) -> (impl Iterator<Item = &TrackedRequest>, u64) {
        let skip = mark.saturating_sub(self.start) as usize;
        let evicted = self.start.saturating_sub(mark.max(self.cleared_through));
        (self.entries.iter().skip(skip), evicted)
    }

    /// Entries evicted since the last `clear`.
    pub fn evicted(&self) -> u64 {
        self.start - self.cleared_through
    }

    /// Whether `request_id` belongs to a recently evicted entry.
    pub fn was_evicted(&self, request_id: &str) -> bool {
        self.evicted_ids.iter().any(|id| id == request_id)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> std::collections::vec_deque::Iter<'_, TrackedRequest> {
        self.entries.iter()
    }

    pub fn iter_mut(&mut self) -> std::collections::vec_deque::IterMut<'_, TrackedRequest> {
        self.entries.iter_mut()
    }
}

impl std::ops::Index<usize> for TrackedRequests {
    type Output = TrackedRequest;
    fn index(&self, i: usize) -> &TrackedRequest {
        &self.entries[i]
    }
}

impl std::ops::IndexMut<usize> for TrackedRequests {
    fn index_mut(&mut self, i: usize) -> &mut TrackedRequest {
        &mut self.entries[i]
    }
}

/// Cap an entry's post data and both header objects, naming each field cut
/// in `truncated`. Response headers arrive after the push, so the response
/// handler calls this again.
pub fn cap_entry(request: &mut TrackedRequest) {
    if let Some(post) = request.post_data.as_mut() {
        if truncate_at_boundary(post, MAX_POST_DATA_BYTES) {
            note_truncated(request, "postData");
        }
    }
    if cap_headers(&mut request.headers, MAX_HEADER_BYTES) {
        note_truncated(request, "headers");
    }
    if let Some(headers) = request.response_headers.as_mut() {
        if cap_headers(headers, MAX_HEADER_BYTES) {
            note_truncated(request, "responseHeaders");
        }
    }
}

fn note_truncated(request: &mut TrackedRequest, field: &'static str) {
    if !request.truncated.contains(&field) {
        request.truncated.push(field);
    }
}

/// Cut `s` to at most `max` bytes on a char boundary. True when it was cut.
fn truncate_at_boundary(s: &mut String, max: usize) -> bool {
    if s.len() <= max {
        return false;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    true
}

/// Keep header names and values, in order, until `max` bytes; the value that
/// crosses the limit is cut and the rest dropped. True when anything was.
fn cap_headers(headers: &mut Value, max: usize) -> bool {
    let Some(map) = headers.as_object_mut() else {
        return false;
    };
    let size = |v: &Value| v.as_str().map_or_else(|| v.to_string().len(), str::len);
    if map.iter().map(|(k, v)| k.len() + size(v)).sum::<usize>() <= max {
        return false;
    }
    let mut remaining = max;
    let mut kept = serde_json::Map::new();
    for (name, value) in std::mem::take(map) {
        let need = name.len() + size(&value);
        if need <= remaining {
            remaining -= need;
            kept.insert(name, value);
            continue;
        }
        if remaining > name.len() {
            if let Value::String(mut s) = value {
                truncate_at_boundary(&mut s, remaining - name.len());
                kept.insert(name, Value::String(s));
            }
        }
        break;
    }
    *map = kept;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(n: u64) -> TrackedRequest {
        TrackedRequest {
            session_id: None,
            url: format!("https://shop.example/api/{n}"),
            method: "GET".into(),
            headers: json!({}),
            timestamp: n,
            resource_type: "Fetch".into(),
            request_id: format!("r{n}"),
            post_data: None,
            status: None,
            response_headers: None,
            mime_type: None,
            truncated: Vec::new(),
        }
    }

    fn ids<'a>(it: impl Iterator<Item = &'a TrackedRequest>) -> Vec<String> {
        it.map(|r| r.request_id.clone()).collect()
    }

    #[test]
    fn eviction_keeps_the_newest_entries() {
        let mut log = TrackedRequests::default();
        for n in 0..(MAX_TRACKED_REQUESTS as u64 + 5) {
            log.push(request(n));
        }
        assert_eq!(log.len(), MAX_TRACKED_REQUESTS);
        assert_eq!(log[0].request_id, "r5");
        assert_eq!(log.evicted(), 5);
        assert_eq!(log.mark(), MAX_TRACKED_REQUESTS as u64 + 5);
        assert!(log.was_evicted("r0") && log.was_evicted("r4"));
        assert!(!log.was_evicted("r5"));
    }

    #[test]
    fn a_mark_survives_eviction_ahead_of_it() {
        let mut log = TrackedRequests::default();
        for n in 0..(MAX_TRACKED_REQUESTS as u64 - 2) {
            log.push(request(n));
        }
        let mark = log.mark();
        // Three more: one evicted from before the mark, none after it.
        for n in 0..3 {
            log.push(request(10_000 + n));
        }
        let (after, evicted) = log.since(mark);
        assert_eq!(ids(after), ["r10000", "r10001", "r10002"]);
        assert_eq!(evicted, 0);
    }

    #[test]
    fn a_mark_overtaken_by_eviction_says_how_many_were_lost() {
        let mut log = TrackedRequests::default();
        log.push(request(0));
        let mark = log.mark();
        let more = MAX_TRACKED_REQUESTS as u64 + 3;
        for n in 1..=more {
            log.push(request(n));
        }
        let (after, evicted) = log.since(mark);
        let after = ids(after);
        // r0 was before the mark; r1..r3 were after it and are gone.
        assert_eq!(evicted, 3);
        assert_eq!(after.len(), MAX_TRACKED_REQUESTS);
        assert_eq!(after[0], "r4");
        assert_eq!(after.last().unwrap(), &format!("r{more}"));
    }

    #[test]
    fn clearing_is_not_eviction_and_keeps_marks_valid() {
        let mut log = TrackedRequests::default();
        log.push(request(0));
        let before_clear = log.mark();
        log.push(request(1));
        log.clear();
        assert!(log.is_empty());
        assert_eq!(log.evicted(), 0);
        assert!(!log.was_evicted("r1"));
        log.push(request(2));
        let (after, evicted) = log.since(before_clear);
        assert_eq!(ids(after), ["r2"]);
        assert_eq!(evicted, 0);
        let (after, _) = log.since(log.mark());
        assert_eq!(after.count(), 0);
    }

    #[test]
    fn post_data_is_capped_on_a_char_boundary() {
        let mut log = TrackedRequests::default();
        let mut r = request(0);
        // Each 'é' is two bytes; an odd prefix puts the cap mid-char.
        r.post_data = Some(format!("x{}", "é".repeat(MAX_POST_DATA_BYTES)));
        log.push(r);
        let post = log[0].post_data.as_deref().unwrap();
        assert!(post.len() <= MAX_POST_DATA_BYTES && post.len() >= MAX_POST_DATA_BYTES - 1);
        assert_eq!(log[0].truncated, ["postData"]);
        let out = serde_json::to_value(&log[0]).unwrap();
        assert_eq!(out["truncated"], json!(["postData"]));

        let mut small = request(1);
        small.post_data = Some("a=1".into());
        log.push(small);
        assert_eq!(log[1].post_data.as_deref(), Some("a=1"));
        assert!(serde_json::to_value(&log[1])
            .unwrap()
            .get("truncated")
            .is_none());
    }

    #[test]
    fn headers_are_capped_in_order() {
        let mut r = request(0);
        r.headers = json!({
            "accept": "*/*",
            "cookie": "c".repeat(MAX_HEADER_BYTES * 2),
            "x-after": "dropped",
        });
        r.response_headers = Some(json!({"set-cookie": "s".repeat(MAX_HEADER_BYTES + 1)}));
        cap_entry(&mut r);
        let h = r.headers.as_object().unwrap();
        assert_eq!(h["accept"], "*/*");
        assert_eq!(
            h["cookie"].as_str().unwrap().len(),
            MAX_HEADER_BYTES - "accept".len() - 3 - "cookie".len()
        );
        assert!(h.get("x-after").is_none());
        let rh = r.response_headers.as_ref().unwrap();
        assert_eq!(
            rh["set-cookie"].as_str().unwrap().len(),
            MAX_HEADER_BYTES - "set-cookie".len()
        );
        assert_eq!(r.truncated, ["headers", "responseHeaders"]);
        // A second pass (the response handler's) adds nothing twice.
        cap_entry(&mut r);
        assert_eq!(r.truncated, ["headers", "responseHeaders"]);
    }
}

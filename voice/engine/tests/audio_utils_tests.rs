//! Tests for audio utilities — ring buffer, constants, frame alignment.

use voice_engine::utils::*;

// ── Constants ────────────────────────────────────────────────────

#[test]
fn constants_are_consistent() {
    assert_eq!(FRAME_BYTES, FRAME_SIZE * SAMPLE_WIDTH);
    assert_eq!(SAMPLE_WIDTH, 2); // PCM16
    assert_eq!(FRAME_SIZE, 512); // Silero VAD requirement
    assert_eq!(SAMPLE_RATE, 16_000);
}

// ── Ring Buffer: basic operations ────────────────────────────────

#[test]
fn ring_buffer_empty_input() {
    let mut rb = AudioRingBuffer::new(4096, FRAME_BYTES);
    let frames = rb.add(&[]);
    assert!(frames.is_empty());
}

#[test]
fn ring_buffer_less_than_one_frame() {
    let mut rb = AudioRingBuffer::new(4096, FRAME_BYTES);
    let data = vec![0u8; FRAME_BYTES - 1];
    let frames = rb.add(&data);
    assert!(frames.is_empty(), "partial frame should not be emitted");
}

#[test]
fn ring_buffer_exact_one_frame() {
    let mut rb = AudioRingBuffer::new(4096, FRAME_BYTES);
    let data = vec![42u8; FRAME_BYTES];
    let frames = rb.add(&data);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].len(), FRAME_BYTES);
    assert!(frames[0].iter().all(|&b| b == 42));
}

#[test]
fn ring_buffer_two_frames_exact() {
    let mut rb = AudioRingBuffer::new(8192, FRAME_BYTES);
    let data = vec![1u8; FRAME_BYTES * 2];
    let frames = rb.add(&data);
    assert_eq!(frames.len(), 2);
}

#[test]
fn ring_buffer_accumulates_across_calls() {
    let mut rb = AudioRingBuffer::new(4096, FRAME_BYTES);

    // First call: half a frame
    let half = vec![0u8; FRAME_BYTES / 2];
    let frames = rb.add(&half);
    assert!(frames.is_empty());

    // Second call: other half
    let frames = rb.add(&half);
    assert_eq!(frames.len(), 1, "two halves should form one frame");
    assert_eq!(frames[0].len(), FRAME_BYTES);
}

#[test]
fn ring_buffer_remainder_preserved() {
    let mut rb = AudioRingBuffer::new(4096, FRAME_BYTES);
    // 1.5 frames
    let data = vec![0u8; FRAME_BYTES + FRAME_BYTES / 2];
    let frames = rb.add(&data);
    assert_eq!(frames.len(), 1);

    // Add another half frame to complete frame #2
    let half = vec![0u8; FRAME_BYTES / 2];
    let frames = rb.add(&half);
    assert_eq!(frames.len(), 1, "remainder + half should form a frame");
}

#[test]
fn ring_buffer_clear_discards_buffered() {
    let mut rb = AudioRingBuffer::new(4096, FRAME_BYTES);
    rb.add(&vec![0u8; FRAME_BYTES / 2]);
    rb.clear();

    // Half frame was discarded, so even another half shouldn't form a frame
    let frames = rb.add(&vec![0u8; FRAME_BYTES / 2]);
    assert!(frames.is_empty());
}

#[test]
fn ring_buffer_many_small_writes() {
    let mut rb = AudioRingBuffer::new(4096, 4); // 4 bytes per frame
    let mut total_frames = 0;

    // Write 1 byte at a time, 20 times
    for _ in 0..20 {
        let frames = rb.add(&[0xAB]);
        total_frames += frames.len();
    }
    assert_eq!(total_frames, 5, "20 bytes / 4 bytes per frame = 5 frames");
}

#[test]
fn ring_buffer_large_write_compacts() {
    // Small capacity buffer to force compaction
    let mut rb = AudioRingBuffer::new(256, 4);

    // Fill and drain multiple times to exercise compaction
    for _ in 0..100 {
        let frames = rb.add(&[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(frames.len(), 2);
    }
}

#[test]
fn ring_buffer_default_uses_frame_bytes() {
    let mut rb = AudioRingBuffer::default();
    let data = vec![0u8; FRAME_BYTES];
    let frames = rb.add(&data);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].len(), FRAME_BYTES);
}

// ── Ring Buffer: data integrity ──────────────────────────────────

#[test]
fn ring_buffer_preserves_data_content() {
    let mut rb = AudioRingBuffer::new(4096, 4);

    // Write two distinguishable frames
    let frames = rb.add(&[10, 20, 30, 40, 50, 60, 70, 80]);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0], vec![10, 20, 30, 40]);
    assert_eq!(frames[1], vec![50, 60, 70, 80]);
}

#[test]
fn ring_buffer_cross_call_data_integrity() {
    let mut rb = AudioRingBuffer::new(4096, 4);

    // Write 3 bytes, then 5 bytes — should get 2 frames
    let frames1 = rb.add(&[1, 2, 3]);
    assert!(frames1.is_empty());

    let frames2 = rb.add(&[4, 5, 6, 7, 8]);
    assert_eq!(frames2.len(), 2);
    assert_eq!(frames2[0], vec![1, 2, 3, 4]); // crosses the boundary
    assert_eq!(frames2[1], vec![5, 6, 7, 8]);
}

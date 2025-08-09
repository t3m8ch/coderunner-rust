use std::thread;
use std::time::{Duration, Instant};

// Touch pages so they are actually backed by physical memory (affect RSS)
fn touch(buf: &mut [u8]) {
    // Simple per-page writes (~4 KiB) to ensure pages are committed without too much overhead
    for chunk in buf.chunks_mut(4096) {
        for b in chunk {
            *b = b.wrapping_add(0x5A);
        }
    }
}

fn mb(n: usize) -> usize {
    n * 1024 * 1024
}

fn main() {
    // Sensible defaults
    let target_peak_mb: usize = 64;   // target peak
    let baseline_mb: usize = 8;       // background baseline
    let wave_mb: usize = 16;          // oscillation amplitude on top of baseline
    let step_ms: u64 = 80;            // background adjustment step
    let hold_peak_ms: u64 = 250;      // how long to hold the peak
    let total_runtime_ms: u64 = 1500; // total program runtime ~1.5s

    // State
    let mut background: Vec<Vec<u8>> = Vec::new();
    let mut peak_blocks: Vec<Vec<u8>> = Vec::new();

    // Small warmup
    {
        let mut warm = vec![0u8; mb(4)];
        touch(&mut warm);
    }

    let start = Instant::now();
    let period_ms: u64 = 400; // period of the “triangular” wave
    let mut did_peak = false;

    // Helpers for block-wise alloc/free
    let bg_block_mb = 4usize;   // background blocks of 4 MB
    let peak_block_mb = 8usize; // peak blocks of 8 MB

    while start.elapsed() < Duration::from_millis(total_runtime_ms) {
        let elapsed_ms = start.elapsed().as_millis() as u64;

        // Triangular wave for background within [baseline, baseline+wave]
        let phase = (elapsed_ms % period_ms) as f64 / period_ms as f64;
        let tri = if phase < 0.5 { phase * 2.0 } else { 2.0 - phase * 2.0 };
        let target_bg_mb = baseline_mb + (tri * wave_mb as f64).round() as usize;

        // Adjust background to target
        let mut current_bg_mb = background.iter().map(|b| b.len()).sum::<usize>() / mb(1);
        if current_bg_mb < target_bg_mb {
            let to_add = target_bg_mb - current_bg_mb;
            let blocks = (to_add + bg_block_mb - 1) / bg_block_mb;
            for _ in 0..blocks {
                let mut buf = vec![0u8; mb(bg_block_mb)];
                touch(&mut buf);
                background.push(buf);
            }
        } else if current_bg_mb > target_bg_mb {
            let to_remove = current_bg_mb - target_bg_mb;
            let blocks = (to_remove + bg_block_mb - 1) / bg_block_mb;
            for _ in 0..blocks {
                if let Some(mut buf) = background.pop() {
                    // Light zeroing
                    for b in buf.iter_mut() { *b = 0; }
                } else {
                    break;
                }
            }
        }
        current_bg_mb = background.iter().map(|b| b.len()).sum::<usize>() / mb(1);

        // Hit the ~64 MB peak once during runtime
        if !did_peak && elapsed_ms > period_ms / 2 && elapsed_ms < total_runtime_ms.saturating_sub(hold_peak_ms / 2) {
            // Clear any leftovers
            peak_blocks.clear();

            let current_total_mb = current_bg_mb;
            if current_total_mb < target_peak_mb {
                let need = target_peak_mb - current_total_mb;
                let blocks = (need + peak_block_mb - 1) / peak_block_mb;
                for _ in 0..blocks {
                    let mut buf = vec![0u8; mb(peak_block_mb)];
                    touch(&mut buf);
                    peak_blocks.push(buf);
                }
            }

            // Hold peak
            thread::sleep(Duration::from_millis(hold_peak_ms));

            // Drop peak blocks
            for mut buf in peak_blocks.drain(..) {
                for b in buf.iter_mut() { *b = 0; }
            }

            did_peak = true;
        }

        thread::sleep(Duration::from_millis(step_ms));
    }

    // Cleanly release background before exit
    for mut buf in background.drain(..) {
        for b in buf.iter_mut() { *b = 0; }
    }
}

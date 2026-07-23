use bevy_assets_hmr::diff_entries_by_id;
use std::hint::black_box;
use std::time::{Duration, Instant};

#[derive(Clone, PartialEq)]
struct Entry {
    id: u32,
    value: u64,
    payload: [u8; 32],
}

fn entries(count: usize) -> Vec<Entry> {
    (0..count)
        .map(|index| Entry {
            id: index as u32,
            value: index as u64,
            payload: [index as u8; 32],
        })
        .collect()
}

fn median_diff_time(count: usize, samples: usize) -> Duration {
    let old = entries(count);
    let mut new = old.clone();
    new[count / 2].value += 1;
    let mut timings = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let delta = diff_entries_by_id(black_box(&old), black_box(&new), |entry| entry.id);
        black_box(delta);
        timings.push(started.elapsed());
    }
    timings.sort_unstable();
    timings[timings.len() / 2]
}

fn sequential_save_time(count: usize, saves: usize) -> Duration {
    let mut old = entries(count);
    let started = Instant::now();
    for save in 0..saves {
        let mut new = old.clone();
        new[save % count].value += 1;
        let delta = diff_entries_by_id(black_box(&old), black_box(&new), |entry| entry.id);
        black_box(delta);
        old = new;
    }
    started.elapsed()
}

fn main() {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    println!("bevy_assets_hmr diff baseline ({profile} build)");
    println!("entries\tmedian\tthroughput");
    for (count, samples) in [(1_000, 10), (10_000, 5), (100_000, 3)] {
        let elapsed = median_diff_time(count, samples);
        let throughput = count as f64 / elapsed.as_secs_f64();
        println!("{count}\t{elapsed:?}\t{throughput:.0} entries/s");
    }

    let saves = 20;
    let elapsed = sequential_save_time(10_000, saves);
    println!(
        "10k sequential saves\t{saves} diffs in {elapsed:?}\t{:.1} saves/s",
        saves as f64 / elapsed.as_secs_f64()
    );
}

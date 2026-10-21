//! Reproducible synthetic workload; includes JSON serialization cost.
use fast_search::Index;
use std::{hint::black_box, time::Instant};

fn main() {
    let index = Index::from_paths(
        (0..200_000)
            .map(|n| (format!("/corpus/group{}/item{n:06}.txt", n % 100), false))
            .collect(),
    );
    for query in [
        "item000*", "item100*", "item199*", "zzz*", "*.txt", "199999",
    ] {
        for _ in 0..10 {
            black_box(index.search_json(query, 50));
        }
        let start = Instant::now();
        for _ in 0..200 {
            black_box(index.search_json(black_box(query), 50));
        }
        println!(
            "{query}: {:.2} us/query",
            start.elapsed().as_secs_f64() * 1e6 / 200.0
        );
    }
}

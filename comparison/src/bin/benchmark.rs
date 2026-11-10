//! Exact-CST-gated latency and allocation measurements. See comparison/README.md.
use std::hint::black_box;
use std::time::{Duration, Instant};
use zsyn::{parse, Diagnostic, Limits, Mode, Node, OwnedTree};
use zsyn_comparison::{
    canonical, compare_records, fnv1a, oracle_canonical, oracle_parse, workloads, ORACLE_VERSION,
};

#[derive(Clone, Copy, Default)]
struct AllocationStats {
    count: usize,
    bytes: usize,
    peak: usize,
}
#[cfg(feature = "allocation-stats")]
mod counting {
    use super::AllocationStats;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    struct CountingAllocator;
    static ENABLED: AtomicBool = AtomicBool::new(false);
    static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
    static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);

    fn count_allocate(bytes: usize) {
        if ENABLED.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED.fetch_add(bytes, Ordering::Relaxed);
            let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
    }
    fn count_free(bytes: usize) {
        if ENABLED.load(Ordering::Relaxed) {
            let mut before = LIVE.load(Ordering::Relaxed);
            loop {
                match LIVE.compare_exchange_weak(
                    before,
                    before.saturating_sub(bytes),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(current) => before = current,
                }
            }
        }
    }
    // This instrumentation is confined to this unpublished comparison executable.
    // Each method delegates to System with the exact original pointer/layout contract.
    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let pointer = unsafe { System.alloc(layout) };
            if !pointer.is_null() {
                count_allocate(layout.size());
            }
            pointer
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let pointer = unsafe { System.alloc_zeroed(layout) };
            if !pointer.is_null() {
                count_allocate(layout.size());
            }
            pointer
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            count_free(layout.size());
            unsafe { System.dealloc(pointer, layout) };
        }
        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let result = unsafe { System.realloc(pointer, layout, new_size) };
            if !result.is_null() {
                count_free(layout.size());
                count_allocate(new_size);
            }
            result
        }
    }
    #[global_allocator]
    static GLOBAL: CountingAllocator = CountingAllocator;

    pub(super) fn allocations<T>(f: impl FnOnce() -> T) -> (T, AllocationStats) {
        ALLOCATIONS.store(0, Ordering::Relaxed);
        ALLOCATED.store(0, Ordering::Relaxed);
        LIVE.store(0, Ordering::Relaxed);
        PEAK.store(0, Ordering::Relaxed);
        ENABLED.store(true, Ordering::SeqCst);
        let value = f();
        ENABLED.store(false, Ordering::SeqCst);
        (
            value,
            AllocationStats {
                count: ALLOCATIONS.load(Ordering::Relaxed),
                bytes: ALLOCATED.load(Ordering::Relaxed),
                peak: PEAK.load(Ordering::Relaxed),
            },
        )
    }

    pub(super) fn edit_allocations<T, R>(
        setup: impl FnOnce() -> T,
        edit: impl FnOnce(&mut T) -> R,
    ) -> (T, R, AllocationStats) {
        ALLOCATIONS.store(0, Ordering::Relaxed);
        ALLOCATED.store(0, Ordering::Relaxed);
        LIVE.store(0, Ordering::Relaxed);
        PEAK.store(0, Ordering::Relaxed);
        ENABLED.store(true, Ordering::SeqCst);
        let mut value = setup();
        ALLOCATIONS.store(0, Ordering::Relaxed);
        ALLOCATED.store(0, Ordering::Relaxed);
        PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
        let result = edit(&mut value);
        ENABLED.store(false, Ordering::SeqCst);
        let stats = AllocationStats {
            count: ALLOCATIONS.load(Ordering::Relaxed),
            bytes: ALLOCATED.load(Ordering::Relaxed),
            peak: PEAK.load(Ordering::Relaxed),
        };
        (value, result, stats)
    }
}
#[cfg(feature = "allocation-stats")]
use counting::{allocations, edit_allocations};
#[cfg(not(feature = "allocation-stats"))]
fn allocations<T>(f: impl FnOnce() -> T) -> (T, AllocationStats) {
    (f(), AllocationStats::default())
}
#[cfg(not(feature = "allocation-stats"))]
fn edit_allocations<T, R>(
    setup: impl FnOnce() -> T,
    edit: impl FnOnce(&mut T) -> R,
) -> (T, R, AllocationStats) {
    let mut value = setup();
    let result = edit(&mut value);
    (value, result, AllocationStats::default())
}

fn median_ns(mut f: impl FnMut()) -> f64 {
    if cfg!(feature = "allocation-stats") {
        f();
        return f64::NAN;
    }
    for _ in 0..8 {
        f();
    }
    let mut iterations = 1u64;
    loop {
        let start = Instant::now();
        for _ in 0..iterations {
            f();
        }
        if start.elapsed() >= Duration::from_millis(15) || iterations >= 1 << 24 {
            break;
        }
        iterations *= 2;
    }
    let mut samples = [0.0; 9];
    for sample in &mut samples {
        let start = Instant::now();
        for _ in 0..iterations {
            f();
        }
        *sample = start.elapsed().as_nanos() as f64 / iterations as f64;
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples[4]
}

fn first_edit_ns<T>(mut setup: impl FnMut() -> T, mut edit: impl FnMut(&mut T)) -> f64 {
    if cfg!(feature = "allocation-stats") {
        edit(&mut setup());
        return f64::NAN;
    }
    let mut samples = [0.0; 9];
    for sample in &mut samples {
        let mut fresh: Vec<T> = (0..8).map(|_| setup()).collect();
        let start = Instant::now();
        for tree in &mut fresh {
            edit(black_box(tree));
        }
        *sample = start.elapsed().as_nanos() as f64 / fresh.len() as f64;
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples[4]
}

struct Row<'a> {
    name: &'a str,
    operation: &'a str,
    engine: &'a str,
    bytes: usize,
    nodes: usize,
    ns: f64,
    reference_ns: f64,
    alloc: AllocationStats,
    arena: usize,
    reserved: usize,
    reparsed: usize,
    hash: u64,
}
fn print_row(row: Row<'_>) {
    let instrumented = cfg!(feature = "allocation-stats");
    let times = if instrumented {
        ",,".to_owned()
    } else {
        let mib_s = row.bytes as f64 / (1024.0 * 1024.0) / (row.ns / 1e9);
        format!(
            "{:.2},{:.2},{:.3}",
            row.ns,
            mib_s,
            row.reference_ns / row.ns
        )
    };
    let memory = if instrumented {
        format!("{},{},{}", row.alloc.count, row.alloc.bytes, row.alloc.peak)
    } else {
        ",,".to_owned()
    };
    println!(
        "{},{},{},true,{},{},{},{},{},{},{},{},{:016x}",
        row.name,
        row.operation,
        row.engine,
        instrumented,
        row.bytes,
        row.nodes,
        times,
        memory,
        row.arena,
        row.reserved,
        row.reparsed,
        row.hash
    );
}

fn main() {
    eprintln!(
        "measurement={}",
        if cfg!(feature = "allocation-stats") {
            "memory"
        } else {
            "timing"
        }
    );
    eprintln!("oracle=typst-syntax {ORACLE_VERSION}; samples=9 median; calibration>=15ms; source bytes use UTF-8");
    eprintln!("core arena creation is outside timed region; owned includes source ownership/allocation; full parse results are dropped inside timing");
    eprintln!("allocation byte counts are requested bytes, not RSS; edit allocation counts exclude construction; edit peaks include the retained document");
    eprintln!("allocation_instrumented={}; first edits use 8 fresh independently parsed trees per sample outside timing", cfg!(feature = "allocation-stats"));
    println!("workload,operation,engine,equivalent,allocation_instrumented,bytes,nodes,median_ns,mib_s,speedup_vs_typst,heap_allocations,allocated_bytes,peak_live_bytes,retained_arena_bytes,reserved_bytes,reparsed_bytes,source_fnv1a");
    let limits = Limits::default();
    for workload in workloads() {
        let source = workload.source.as_str();
        let oracle = oracle_parse(source, Mode::Markup);
        let ours = OwnedTree::parse(source, Mode::Markup, limits).unwrap();
        let records = canonical(ours.as_tree());
        if let Err(error) = compare_records(&records, &oracle_canonical(&oracle)) {
            eprintln!(
                "{}: benchmark withheld: full CST mismatch: {error}",
                workload.name
            );
            let mut fields = vec![String::new(); 17];
            fields[0] = workload.name.to_owned();
            fields[1] = "full".to_owned();
            fields[2] = "none".to_owned();
            fields[3] = "false".to_owned();
            fields[4] = cfg!(feature = "allocation-stats").to_string();
            fields[5] = source.len().to_string();
            fields[16] = format!("{:016x}", fnv1a(source.as_bytes()));
            println!("{}", fields.join(","));
            continue;
        }
        let node_count = ours.as_tree().nodes().len();
        let arena_bytes = ours.as_tree().storage_bytes();
        let mut nodes = vec![Node::EMPTY; node_count + 64];
        let mut diagnostics = vec![Diagnostic::EMPTY; ours.as_tree().diagnostics().len() + 16];
        let reserved = nodes.len() * std::mem::size_of::<Node>()
            + diagnostics.len() * std::mem::size_of::<Diagnostic>();
        let reference_ns = median_ns(|| {
            black_box(oracle_parse(black_box(source), Mode::Markup));
        });
        let owned_ns = median_ns(|| {
            black_box(OwnedTree::parse(black_box(source), Mode::Markup, limits).unwrap());
        });
        let arena_ns = median_ns(|| {
            black_box(
                parse(
                    black_box(source),
                    Mode::Markup,
                    &mut nodes,
                    &mut diagnostics,
                    limits,
                )
                .unwrap(),
            );
        });
        let (_, ref_alloc) = allocations(|| oracle_parse(black_box(source), Mode::Markup));
        let (owned_sample, owned_alloc) =
            allocations(|| OwnedTree::parse(black_box(source), Mode::Markup, limits).unwrap());
        let (_, arena_alloc) = allocations(|| {
            parse(
                black_box(source),
                Mode::Markup,
                &mut nodes,
                &mut diagnostics,
                limits,
            )
            .unwrap()
        });
        let hash = fnv1a(source.as_bytes());
        for (engine, ns, stats, arena, reserve) in [
            ("typst", reference_ns, ref_alloc, 0, 0),
            (
                "zsyn-owned",
                owned_ns,
                owned_alloc,
                arena_bytes,
                owned_sample.storage_capacity_bytes(),
            ),
            ("zsyn-arena", arena_ns, arena_alloc, arena_bytes, reserved),
        ] {
            print_row(Row {
                name: workload.name,
                operation: "full",
                engine,
                bytes: source.len(),
                nodes: node_count,
                ns,
                reference_ns,
                alloc: stats,
                arena,
                reserved: reserve,
                reparsed: source.len(),
                hash,
            });
        }
        let at = workload.edit_at;
        let original = &source[at..at + 1];
        let replacement = if original == "q" { "r" } else { "q" };
        let mut changed = source.to_owned();
        changed.replace_range(at..at + 1, replacement);
        let full_after = OwnedTree::parse(&changed, Mode::Markup, limits).unwrap();
        let oracle_after = oracle_parse(&changed, Mode::Markup);
        if let Err(error) = compare_records(
            &canonical(full_after.as_tree()),
            &oracle_canonical(&oracle_after),
        ) {
            eprintln!(
                "{}: edit benchmark withheld: edited CST mismatch: {error}",
                workload.name
            );
            continue;
        }
        let mut zsyn_edit = OwnedTree::parse(source, Mode::Markup, limits).unwrap();
        let mut typst_edit = typst_syntax::Source::detached(source);
        let zstats = zsyn_edit.edit(at..at + 1, replacement, limits).unwrap();
        let trange = typst_edit.edit(at..at + 1, replacement);
        assert_eq!(
            canonical(zsyn_edit.as_tree()),
            canonical(full_after.as_tree())
        );
        assert_eq!(
            oracle_canonical(typst_edit.root()),
            oracle_canonical(&oracle_after)
        );
        let first_reference_ns = first_edit_ns(
            || typst_syntax::Source::detached(source),
            |tree| {
                black_box(tree.edit(black_box(at..at + 1), black_box(replacement)));
            },
        );
        let first_ns = first_edit_ns(
            || OwnedTree::parse(source, Mode::Markup, limits).unwrap(),
            |tree| {
                black_box(
                    tree.edit(black_box(at..at + 1), black_box(replacement), limits)
                        .unwrap(),
                );
            },
        );
        let mut flip = false;
        let hot_reference_ns = median_ns(|| {
            let text = if flip { replacement } else { original };
            flip = !flip;
            black_box(typst_edit.edit(black_box(at..at + 1), black_box(text)));
        });
        let mut flip = false;
        let hot_ns = median_ns(|| {
            let text = if flip { replacement } else { original };
            flip = !flip;
            black_box(
                zsyn_edit
                    .edit(black_box(at..at + 1), black_box(text), limits)
                    .unwrap(),
            );
        });
        let (zsyn_sample, _, zalloc_first) = edit_allocations(
            || OwnedTree::parse(source, Mode::Markup, limits).unwrap(),
            |tree| tree.edit(at..at + 1, replacement, limits).unwrap(),
        );
        let (_, _, talloc_first) = edit_allocations(
            || typst_syntax::Source::detached(source),
            |tree| tree.edit(at..at + 1, replacement),
        );
        let (_, _, zalloc_hot) = edit_allocations(
            || {
                let mut tree = OwnedTree::parse(source, Mode::Markup, limits).unwrap();
                tree.edit(at..at + 1, replacement, limits).unwrap();
                tree
            },
            |tree| tree.edit(at..at + 1, original, limits).unwrap(),
        );
        let (_, _, talloc_hot) = edit_allocations(
            || {
                let mut tree = typst_syntax::Source::detached(source);
                tree.edit(at..at + 1, replacement);
                tree
            },
            |tree| tree.edit(at..at + 1, original),
        );
        for (operation, engine, ns, reference_ns, stats, arena, reserve, reparsed) in [
            (
                "edit-first",
                "typst-Source",
                first_reference_ns,
                first_reference_ns,
                talloc_first,
                0,
                0,
                trange.len(),
            ),
            (
                "edit-first",
                "zsyn-OwnedTree",
                first_ns,
                first_reference_ns,
                zalloc_first,
                zsyn_sample.as_tree().storage_bytes(),
                zsyn_sample.storage_capacity_bytes(),
                zstats.reparsed.len() as usize,
            ),
            (
                "edit-hot",
                "typst-Source",
                hot_reference_ns,
                hot_reference_ns,
                talloc_hot,
                0,
                0,
                trange.len(),
            ),
            (
                "edit-hot",
                "zsyn-OwnedTree",
                hot_ns,
                hot_reference_ns,
                zalloc_hot,
                zsyn_sample.as_tree().storage_bytes(),
                zsyn_sample.storage_capacity_bytes(),
                zstats.reparsed.len() as usize,
            ),
        ] {
            print_row(Row {
                name: workload.name,
                operation,
                engine,
                bytes: source.len(),
                nodes: node_count,
                ns,
                reference_ns,
                alloc: stats,
                arena,
                reserved: reserve,
                reparsed,
                hash,
            });
        }
    }
}

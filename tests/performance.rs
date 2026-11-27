//! Explicit, release-only internal timings. No GPU/display-latency assertions.
use filavim::{
    buffer::Buffer,
    config::{Config, CursorConfig},
    gui::{
        cursor::{Cursor, Geometry},
        documents::Documents,
        model::Model,
        viewport::VisibleLine,
    },
};
use ropey::Rope;
use serde_json::json;
use std::{hint::black_box, time::Instant};
fn measure(name: &str, count: usize, mut sample: impl FnMut(usize)) -> serde_json::Value {
    let mut ns = Vec::with_capacity(count);
    for i in 0..count {
        let at = Instant::now();
        sample(i);
        ns.push(at.elapsed().as_nanos() as u64);
    }
    ns.sort_unstable();
    let percentile = |p: usize| ns[(count * p).div_ceil(100).saturating_sub(1).min(count - 1)];
    json!({"workload":name,"samples":count,"p50_ns":percentile(50),"p95_ns":percentile(95),"p99_ns":percentile(99),"max_ns":ns[count-1]})
}
#[cfg(windows)]
fn resident() -> Option<usize> {
    #[repr(C)]
    struct Counters {
        cb: u32,
        faults: u32,
        peak: usize,
        working: usize,
        paged_peak: usize,
        paged: usize,
        nonpaged_peak: usize,
        nonpaged: usize,
        pagefile: usize,
        pagefile_peak: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, size: u32) -> i32;
    }
    let mut c = Counters {
        cb: std::mem::size_of::<Counters>() as u32,
        faults: 0,
        peak: 0,
        working: 0,
        paged_peak: 0,
        paged: 0,
        nonpaged_peak: 0,
        nonpaged: 0,
        pagefile: 0,
        pagefile_peak: 0,
    };
    // Test-only query of this process; the buffer matches PROCESS_MEMORY_COUNTERS.
    (unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut c,
            std::mem::size_of::<Counters>() as u32,
        )
    } != 0)
        .then_some(c.working)
}
#[cfg(not(windows))]
fn resident() -> Option<usize> {
    None
}

#[test]
#[ignore = "explicit release performance suite; no fixed latency thresholds"]
fn internal_workload_baseline() {
    if cfg!(debug_assertions) {
        eprintln!("Unoptimized run: use --release for comparable baselines");
    }
    let mut b = Buffer::new(
        Rope::from_str(&"let café = 42; // 😀\n".repeat(50_000)),
        None,
    );
    b.cursor = b.text.len_chars() / 2;
    b.snap_cursor();
    let typing = measure("large_buffer_insert_delete_pair", 256, |_| {
        b.insert("x");
        b.backspace();
        black_box(&b.text);
    });
    let vertical = measure("vertical_scroll_40_visible_fragments", 128, |i| {
        let top = i * 19;
        for row in top..top + 40 {
            black_box(VisibleLine::new(&b, row, 0, 120, 4));
        }
    });
    let mut long = Buffer::new(Rope::from_str(&"x".repeat(1_000_000)), None);
    long.cursor = 500_000;
    long.column(4);
    let horizontal = measure("1MiB_line_horizontal_move_and_fragment_warm", 256, |i| {
        long.horizontal(i % 2 == 0);
        black_box(VisibleLine::new(
            &long,
            0,
            long.column(4).saturating_sub(60),
            120,
            4,
        ));
    });
    let mut cursor = Cursor::default();
    let c = CursorConfig::default();
    let animation = measure("cursor_simulation_frame", 256, |i| {
        cursor.track(
            Geometry {
                x: (i % 20) as f32 * 10.,
                y: (i % 8) as f32 * 24.,
                width: 10.,
                height: 24.,
            },
            1. / 60.,
            false,
            &c,
        );
        black_box(cursor.active());
    });
    drop(b);
    drop(long);
    let source = "x".repeat(2 * 1024 * 1024);
    let before = resident();
    let mut config = Config {
        syntax: false,
        ..Default::default()
    };
    config.lsp.enabled = false;
    let mut docs = Documents::new(Model::new(config).unwrap());
    let mut ids = Vec::new();
    for i in 0..12 {
        ids.push(
            docs.add(
                Buffer::new(
                    Rope::from_str(&source),
                    Some(format!("retention-{i}.rs").into()),
                ),
                true,
            )
            .unwrap(),
        );
    }
    let loaded = resident();
    for id in ids {
        docs.close_document(id, true).unwrap();
    }
    let closed = resident();
    let opening = measure("open_close_2MiB_document", 64, |i| {
        let id = docs
            .add(
                Buffer::new(
                    Rope::from_str(&source),
                    Some(format!("repeat-{i}.rs").into()),
                ),
                true,
            )
            .unwrap();
        docs.close_document(id, true).unwrap();
    });
    println!(
        "FILAVIM_METRICS {}",
        json!({"kind":"internal-release","optimized":!cfg!(debug_assertions),"timings":[typing,vertical,horizontal,animation,opening],"resident_bytes":{"before":before,"12_documents_loaded":loaded,"after_close":closed},"limitations":"No GPUI shaping, GPU, terminal or display latency. Warm horizontal checkpoints. RSS includes allocator retention; no leak threshold."})
    );
}

#[test]
#[ignore = "opt-in filesystem/search/recovery workload timings; run --release serially"]
fn stabilization_workloads() {
    use filavim::{
        files,
        recovery::Recovery,
        search::{self, Options},
    };
    let directory = tempfile::tempdir().unwrap();
    for (name, line, target) in [
        ("small", "let needle = 42;\n", 4 * 1024),
        ("typical", "let needle = 42; // comment\n", 128 * 1024),
        (
            "large",
            "let needle = 42; // repeated realistic source\n",
            4 * 1024 * 1024,
        ),
        (
            "unicode",
            "let café = \"日本😀\"; // needle\r\n",
            1024 * 1024,
        ),
    ] {
        let source = line.repeat(target / line.len());
        let rope = Rope::from_str(&source);
        let path = directory.path().join(format!("{name}.rs"));
        std::fs::write(&path, &source).unwrap();
        let mut timings = Vec::new();
        timings.push(measure("open_tracked_including_hashes", 7, |_| {
            black_box(files::open_tracked(&path).unwrap());
        }));
        timings.push(measure("disk_conflict_verification", 7, |_| {
            black_box(files::disk_state(&path).unwrap());
        }));
        timings.push(measure("literal_search", 7, |_| {
            black_box(search::run(&rope, "needle", Options::default(), 0, false, None).unwrap());
        }));
        timings.push(measure("regex_search", 7, |_| {
            black_box(
                search::run(
                    &rope,
                    r"let\s+(\w+)",
                    Options {
                        regex: true,
                        ..Default::default()
                    },
                    0,
                    false,
                    None,
                )
                .unwrap(),
            );
        }));
        timings.push(measure("replace_all", 7, |_| {
            black_box(
                search::run(
                    &rope,
                    "needle",
                    Options::default(),
                    0,
                    false,
                    Some(("replacement", true)),
                )
                .unwrap(),
            );
        }));
        let output = search::run(
            &rope,
            "needle",
            Options::default(),
            0,
            false,
            Some(("replacement", true)),
        )
        .unwrap()
        .replacement
        .unwrap();
        let mut buffer = Buffer::new(rope.clone(), None);
        buffer.replace(0..buffer.text.len_chars(), &output);
        timings.push(measure("undo_redo_large_replacement", 7, |_| {
            assert!(buffer.undo());
            assert!(buffer.redo());
            black_box(&buffer.text);
        }));
        timings.push(measure(
            "twenty_edits_then_one_active_search_rescan",
            7,
            |_| {
                let mut buffer = Buffer::new(rope.clone(), None);
                for _ in 0..20 {
                    buffer.insert("x");
                }
                black_box(
                    search::run(
                        &buffer.text,
                        "needle",
                        Options::default(),
                        buffer.cursor,
                        false,
                        None,
                    )
                    .unwrap(),
                );
            },
        ));
        let mut config = Config::default();
        config.lsp.enabled = false;
        let mut docs = Documents::new(Model::new(config).unwrap());
        let mut dirty = Buffer::new(Rope::new(), None);
        dirty.insert(&source);
        docs.add(dirty, true).unwrap();
        let mut recovery = Recovery::new(directory.path().join(format!("recovery-{name}")));
        timings.push(measure("recovery_snapshot_flush_and_load", 7, |_| {
            docs.editor.buffer.insert("x");
            recovery.sync(&docs);
            recovery
                .flush()
                .recv_timeout(std::time::Duration::from_secs(30))
                .unwrap();
            let file = std::fs::read_dir(directory.path().join(format!("recovery-{name}")))
                .unwrap()
                .flat_map(|entry| std::fs::read_dir(entry.unwrap().path()).unwrap())
                .map(|entry| entry.unwrap().path())
                .find(|path| path.extension().is_some_and(|e| e == "json"))
                .unwrap();
            black_box(filavim::recovery::load(&file).unwrap());
            assert!(
                recovery
                    .events
                    .try_iter()
                    .all(|event| !matches!(event, filavim::recovery::Event::Error(_)))
            );
        }));
        for i in 0..7 {
            docs.add(
                Buffer::new(rope.clone(), Some(format!("switch-{i}.rs").into())),
                false,
            )
            .unwrap();
        }
        let ids = docs.order.clone();
        timings.push(measure("document_switch", 128, |i| {
            assert!(docs.switch(ids[i % ids.len()]));
            black_box(docs.id());
        }));
        println!(
            "FILAVIM_METRICS {}",
            json!({"kind":"stabilization","dataset":name,"bytes":source.len(),"optimized":!cfg!(debug_assertions),"timings":timings,"limitations":"Warm filesystem cache; operation timings, not UI/display latency. Seven samples characterize this host only. Recovery flush bypasses debounce."})
        );
    }
    let source = "é日本😀".repeat(1024 * 1024 / "é日本😀".len());
    let buffer = Buffer::new(Rope::from_str(&source), None);
    let result = measure("long_unicode_visible_fragment", 32, |i| {
        black_box(VisibleLine::new(&buffer, 0, 500_000 + i, 120, 4));
    });
    println!(
        "FILAVIM_METRICS {}",
        json!({"kind":"stabilization","dataset":"long-unicode-line","bytes":source.len(),"optimized":!cfg!(debug_assertions),"timings":[result]})
    );
}

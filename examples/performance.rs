//! Noninteractive, release-mode microbenchmarks. No application/PTY/filesystem workers.
//! Run: cargo run --release --example performance
use filavim::{
    buffer::Buffer,
    config::Config,
    editor::{Editor, Mode},
    input::Keymap,
    theme::Theme,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use ropey::Rope;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};
struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(size as u64, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;
fn measure(name: &str, count: usize, mut run: impl FnMut()) {
    run();
    ALLOCS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    let at = Instant::now();
    for _ in 0..count {
        run();
    }
    println!(
        "{name}: n={count} us/op={:.2} allocations/op={:.2} allocated_bytes/op={:.0}",
        at.elapsed().as_secs_f64() * 1e6 / count as f64,
        ALLOCS.load(Ordering::Relaxed) as f64 / count as f64,
        BYTES.load(Ordering::Relaxed) as f64 / count as f64
    );
}
fn main() {
    let mut editor = Editor::new(Config::default());
    editor.set_buffer(Buffer::new(
        Rope::from_str(&format!(
            "{}\n{}",
            "x".repeat(1_000_000),
            "let value = 42;\n".repeat(80)
        )),
        None,
    ));
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let theme = Theme::default();
    measure("render_1m_line_start", 200, || {
        terminal
            .draw(|f| ui::draw(f, &mut editor, &theme, &[], ""))
            .unwrap();
    });
    let mut map = Keymap::new(&Config::default()).unwrap();
    measure("unbound_insert_key", 100_000, || {
        black_box(map.resolve(
            Mode::Insert,
            crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char('z')),
        ));
    });
    let items = serde_json::json!({"items":(0..10_000).map(|i| serde_json::json!({"label":format!("value{i:05}"),"sortText":format!("{i:05}"),"detail":"detail".repeat(20),"documentation":"docs".repeat(100)})).collect::<Vec<_>>()});
    // Input ownership is part of response parsing in production. Exclude fixture cloning.
    let fixtures: Vec<_> = (0..21).map(|_| items.clone()).collect();
    let mut fixtures = fixtures.into_iter();
    measure("completion_10k", 20, || {
        black_box(filavim::ide::completion_rows(
            fixtures.next().unwrap(),
            "value",
        ));
    });
    let mut parser = vt100::Parser::new(
        std::num::NonZeroU16::new(24).unwrap(),
        std::num::NonZeroU16::new(120).unwrap(),
        2000,
    );
    for _ in 0..2500 {
        parser.process(b"terminal output with scrollback\r\n");
    }
    measure("terminal_legacy_clone", 1000, || {
        black_box(parser.screen().clone());
    });
    measure("terminal_viewport_snapshot", 1000, || {
        black_box(filavim::terminal::ViewportSnapshot::capture(
            parser.screen(),
        ));
    });
}

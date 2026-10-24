#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1).peekable();
    if args.peek().is_some_and(|a| a == "--smoke-test") {
        args.next();
        let report = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("--smoke-test REPORT FILE"))?;
        let path = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("--smoke-test REPORT FILE"))?;
        return filavim::gui::smoke(path.into(), report.into());
    }
    let mut path = None;
    let mut positional = false;
    for arg in args {
        if !positional && arg == "--" {
            positional = true;
            continue;
        }
        if !positional && (arg == "--version" || arg == "-V") {
            println!("filavim-gpui 0.4.0");
            return Ok(());
        }
        if !positional && (arg == "--help" || arg == "-h") {
            println!(
                "Filavim GPUI\nUsage: filavim [FILE]\nCtrl-O opens a file. Ctrl-S saves. :e/:w/:q use existing commands."
            );
            return Ok(());
        }
        if !positional && arg.to_string_lossy().starts_with('-') {
            anyhow::bail!("Unknown option: {}", arg.to_string_lossy());
        }
        if path.replace(std::path::PathBuf::from(arg)).is_some() {
            anyhow::bail!("Checkpoint 1 opens one file at a time");
        }
    }
    filavim::gui::run(path, filavim::config::Config::load()?)
}

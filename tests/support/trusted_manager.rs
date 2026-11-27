pub fn trusted_manager(
    data: std::path::PathBuf,
    workspace: std::path::PathBuf,
) -> filavim::lsp_manager::Manager {
    use filavim::lsp_manager::{Manager, Operation};
    let mut manager = Manager::at(data, workspace);
    assert!(manager.submit(Operation::TrustWorkspace(true)));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while manager.busy {
        assert!(
            std::time::Instant::now() < deadline,
            "Cannot trust test workspace"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
        manager.poll();
    }
    assert!(manager.snapshot.trusted, "{:?}", manager.error);
    manager
}

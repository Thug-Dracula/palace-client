use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use palace_app_lib::logging::{self, Level};

#[test]
fn a_panic_is_logged_and_still_reaches_the_previous_hook() {
    static PREVIOUS_RAN: AtomicBool = AtomicBool::new(false);

    let dir = std::env::temp_dir().join(format!("palace-app-panic-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    std::panic::set_hook(Box::new(|_| {
        PREVIOUS_RAN.store(true, Ordering::SeqCst);
    }));
    logging::init_at(&dir, Level::Debug).expect("the log initializes");

    let caught = std::panic::catch_unwind(|| panic!("the canary panic"));
    assert!(caught.is_err(), "the panic unwinds");
    assert!(
        PREVIOUS_RAN.load(Ordering::SeqCst),
        "the hook that was installed before logging still runs"
    );

    let text: String =
        std::fs::read_to_string(PathBuf::from(&dir).join(logging::LOG_FILE)).expect("the log");
    assert!(text.contains("the canary panic"), "{text}");
    assert!(
        text.contains("log_panics.rs"),
        "the panic location is recorded: {text}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

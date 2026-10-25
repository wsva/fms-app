//! Native diagnostics capture: the process's stderr and its panics, on their way
//! into the app log.
//!
//! Two sinks the `log` facade does not reach by itself, both of which matter
//! exactly when something dies inside the C/C++ half of the STT stack:
//!
//! 1. **The STT library's own sink.** `transcribe_cpp::init_logging()` routes
//!    whisper/ggml diagnostics into `log`; `lib.rs` calls it at startup. Naming
//!    the library's log API is the cheap half of the problem — it cannot cover
//!    the paths that fail *below* it.
//! 2. **The process's stderr.** `GGML_ABORT`, an MSVC `assert`, and a fail-fast
//!    `abort()` all write to stderr and then die without going through any log
//!    API. A Tauri release build is a *GUI-subsystem* binary: launched from
//!    Explorer or the installer it has no console and no valid
//!    `STD_ERROR_HANDLE`, so those bytes go nowhere at all. That is why a failed
//!    Whisper load reads as `Loading model 'whisper-small'` and then silence.
//!    [`install`] gives the process a private pipe in that case and forwards
//!    every line into this app's log, which flushes per entry — so the message
//!    is on disk before the crash is.
//!
//! Both the default panic hook and the C runtime resolve stderr differently (the
//! Win32 std handle vs. the CRT's fd 2), so the bridge rebinds both; see
//! [`imp::install_stderr_bridge`].
//!
//! The pipeline is deliberately dumb — bytes, then lines, then `log::error!` —
//! because it must not be the thing that fails.

use std::sync::Once;

/// Install the panic hook (every platform) and, on Windows, the stderr bridge.
/// Idempotent and safe to call from either `run()`'s setup hook; the first call
/// wins so a second `run()` in the same process cannot stack hooks.
pub fn install() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        install_panic_hook();
        #[cfg(windows)]
        imp::install_stderr_bridge();
    });
}

/// Log a panic before the default hook prints it.
///
/// The default hook writes to stderr, which a GUI build does not have — a panic
/// therefore leaves the same silence an abort does, and the process still tears
/// down (or unwinds out of whatever thread it was on). This runs first, and the
/// logger flushes per entry, so the message is in the file even if the panic is
/// immediately fatal. Interactive runs keep their usual output: the replaced
/// hook is chained last.
fn install_panic_hook() {
    // The hook logs, and the logger unwraps its own mutex. A panic that strikes
    // while that lock is held would therefore recurse hook → log → panic until the
    // stack runs out. One re-entry check keeps the worst case at the default
    // double-panic abort instead of a stack overflow.
    thread_local! {
        static IN_HOOK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if IN_HOOK.with(|flag| flag.replace(true)) {
            previous(info);
            return;
        }
        let payload = info.payload();
        let text = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_string());
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown location".to_string());

        log::error!("[panic] {} (at {})", text, location);
        // The frames are gone by the time anyone reads the log, and a release
        // build may have no symbols — the addresses are still worth keeping.
        // Costs nothing on the happy path: it only runs inside the hook.
        log::error!(
            "[panic] backtrace:\n{}",
            std::backtrace::Backtrace::force_capture()
        );

        previous(info);
        // Covers a panic caught by `catch_unwind`: the next one on this thread
        // must log again.
        IN_HOOK.with(|flag| flag.set(false));
    }));
}

/// Windows: give a console-less process a stderr that reaches this log.
#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::io::Read;
    use std::os::windows::io::{FromRawHandle, RawHandle};

    /// `STD_ERROR_HANDLE`: (-12) reinterpreted as the unsigned form the API takes.
    const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF4;
    /// The CRT's own fd for stderr, which is what `fprintf(stderr, ..)` uses.
    const STDERR_FD: i32 = 2;

    extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn SetStdHandle(which: u32, handle: *mut c_void) -> i32;
        fn CreatePipe(
            read: *mut *mut c_void,
            write: *mut *mut c_void,
            attrs: *mut c_void,
            size: u32,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    // UCRT/MSVCRT exports; resolved from the CRT import library the toolchain
    // already links for std, so no extra dependency and no `#[link]` guess.
    extern "C" {
        fn _get_osfhandle(fd: i32) -> isize;
        fn _open_osfhandle(osfhandle: isize, flags: i32) -> i32;
        fn _dup2(src_fd: i32, dst_fd: i32) -> i32;
        fn _close(fd: i32) -> i32;
    }

    pub(super) fn install_stderr_bridge() {
        // A console — or a parent that redirected us, which is what
        // `pnpm tauri dev` gives the child — already has somewhere for these
        // bytes to go. Replacing it would hide them from whoever is watching, so
        // the bridge only fills the hole. (Debug builds are console-subsystem
        // binaries and therefore always take this path; only the release shape —
        // `windows_subsystem = "windows"` — actually needs the pipe.)
        let current = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
        if !current.is_null() && current != (-1isize as *mut c_void) {
            log::debug!("[native-log] stderr is already connected; leaving it alone");
            return;
        }

        let (mut read, mut write) = (std::ptr::null_mut(), std::ptr::null_mut());
        // Null SECURITY_ATTRIBUTES: the handles are not inheritable, so a child
        // process (the Python script runner, a sidecar) cannot keep the write end
        // open and hold the reader thread hostage.
        if unsafe { CreatePipe(&mut read, &mut write, std::ptr::null_mut(), 0) } == 0 {
            log::warn!("[native-log] CreatePipe failed; native stderr stays invisible");
            return;
        }

        // Two redirects for two consumers, and both are needed:
        //  * the CRT's fd 2 — what the C/C++ side's `fprintf(stderr, ..)` writes
        //    to (ggml's abort path, MSVC asserts);
        //  * the Win32 std handle — what Rust's own `std::io::stderr()` resolves,
        //    i.e. the default panic hook and any `eprintln!`.
        let fd = unsafe { _open_osfhandle(write as isize, 0) };
        if fd < 0 {
            log::warn!("[native-log] _open_osfhandle failed; native stderr stays invisible");
            unsafe { CloseHandle(write) };
            unsafe { CloseHandle(read) };
            return;
        }
        if unsafe { _dup2(fd, STDERR_FD) } < 0 {
            log::warn!("[native-log] _dup2 failed; native stderr stays invisible");
            unsafe { _close(fd) };
            unsafe { CloseHandle(read) };
            return;
        }
        // `_dup2` took its own reference to the handle, so the temporary fd goes.
        unsafe { _close(fd) };
        if unsafe { SetStdHandle(STD_ERROR_HANDLE, _get_osfhandle(STDERR_FD) as *mut c_void) } == 0 {
            // The CRT is already redirected, so the C side is still covered; this
            // only means Rust's own stderr (the default panic hook) is not.
            log::warn!("[native-log] SetStdHandle failed; only CRT stderr is bridged");
        }

        // Whoever writes now writes into a fixed-size pipe, so something has to
        // drain it or the writer blocks. Its own thread, owned for the process
        // lifetime (the read end closes only at exit).
        let mut pipe = unsafe { std::fs::File::from_raw_handle(read as RawHandle) };
        let spawned = std::thread::Builder::new()
            .name("native-stderr".to_string())
            .spawn(move || forward(&mut pipe));
        if spawned.is_err() {
            log::error!("[native-log] could not start the stderr reader thread");
            return;
        }
        log::info!("[native-log] stderr is now forwarded into the app log");
    }

    /// Pipe bytes to log entries, one per line.
    fn forward(pipe: &mut std::fs::File) {
        let mut pending: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = match pipe.read(&mut buf) {
                // Write end gone: the process is shutting down.
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            pending.extend_from_slice(&buf[..n]);
            while let Some(i) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = pending.drain(..=i).collect();
                emit(&line);
            }
        }
        // A crash can cut the last line off before its newline. Those bytes are
        // usually the ones that explain it, so they are still worth having.
        if !pending.is_empty() {
            emit(&pending);
        }
    }

    fn emit(raw: &[u8]) {
        let text = String::from_utf8_lossy(raw);
        let text = text.trim_end_matches(['\r', '\n']);
        if text.trim().is_empty() {
            return;
        }
        log::error!(target: "native", "{}", text);
    }
}

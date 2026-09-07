# audiowaveform
The resolution logic is now:
System PATH — runs audiowaveform --version to check if it's available. If yes, uses "audiowaveform" directly.
Bundled sidecar — scans the Tauri resource directory for audiowaveform-*. If found, uses that path.
Error — if neither is found, returns a clear message telling the user to install audiowaveform or place the binary in src-tauri/binaries/.


Windows x64	audiowaveform-x86_64-pc-windows-msvc.exe
macOS Intel	audiowaveform-x86_64-apple-darwin
macOS Apple Silicon	audiowaveform-aarch64-apple-darwin
Linux x64	audiowaveform-x86_64-unknown-linux-gnu
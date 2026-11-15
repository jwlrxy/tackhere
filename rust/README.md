# TackHere Windows build

TackHere is an interaction redesign of PinIt v3.0.0 by Saqlain Abbas. The modified Rust / Slint implementation in this directory builds the current Windows x64 application.

## Build

Use stable Rust with the `x86_64-pc-windows-msvc` toolchain, Visual Studio C++ build tools, and a Windows SDK. From the repository root:

```powershell
cargo test --manifest-path rust/Cargo.toml --locked
cargo build --manifest-path rust/Cargo.toml --release --locked
```

The executable is produced at `rust/target/release/tackhere.exe`. The release asset is named `TackHere.exe`.

`TACKHERE_DEV=1` opens an independent test instance and stores its settings in `%LOCALAPPDATA%\TackHere-dev`. The normal program stores settings and pins in `%LOCALAPPDATA%\TackHere`.

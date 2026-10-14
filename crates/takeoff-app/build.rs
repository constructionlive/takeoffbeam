//! Windows: embed the app icon and version info in takeoff-desk.exe.
fn main() {
	println!("cargo:rerun-if-changed=../../assets/icon.ico");
	if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
		return;
	}
	let mut res = winresource::WindowsResource::new();
	if std::env::var("HOST").is_ok_and(|h| !h.contains("windows")) {
		// Cross-compiling from macOS/Linux with mingw.
		res.set_toolkit_path("/opt/homebrew/bin").set_windres_path("x86_64-w64-mingw32-windres").set_ar_path("x86_64-w64-mingw32-ar");
	}
	res.set_icon("../../assets/icon.ico")
		.set("ProductName", "Takeoff Desk")
		.set("FileDescription", "Takeoff Desk — takeoff with a native AI agent")
		.set("CompanyName", "Neuratwin Inc. (construction.live)")
		.set("LegalCopyright", "© 2026 Neuratwin Inc. · Apache-2.0");
	if let Err(e) = res.compile() {
		println!("cargo:warning=could not embed the Windows icon: {e}");
	}
}

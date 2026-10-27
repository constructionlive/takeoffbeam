#!/usr/bin/env node
// Starts Takeoff Beam's MCP server (`takeoff mcp`) for Claude Code.
//
// The plugin manifest can name one command, and the binary lives in a
// different place on every platform: inside the .app bundle on macOS, in
// Program Files on Windows, on PATH when built from source. So the manifest
// runs this launcher (node is always present where Claude Code runs) and the
// launcher finds the binary, then hands its stdio straight through.
//
// Override with TAKEOFF_BIN=/path/to/takeoff.

"use strict";

const { spawn, spawnSync } = require("node:child_process");
const { existsSync } = require("node:fs");
const { join } = require("node:path");
const os = require("node:os");

const APP_NAMES = ["Takeoff Beam", "Takeoff Desk"];

function onPath(name) {
	const probe = process.platform === "win32" ? "where" : "which";
	const r = spawnSync(probe, [name], { encoding: "utf8" });
	if (r.status !== 0) return null;
	const first = r.stdout.split(/\r?\n/).find((l) => l.trim());
	return first ? first.trim() : null;
}

function windowsInstallDir() {
	// The installer records where it put the app; respect a custom location.
	const r = spawnSync("reg", ["query", "HKLM\\Software\\Takeoff Desk", "/v", "InstallDir"], { encoding: "utf8" });
	if (r.status !== 0) return null;
	const m = r.stdout.match(/InstallDir\s+REG_SZ\s+(.+)/);
	return m ? m[1].trim() : null;
}

function candidates() {
	const list = [];
	if (process.env.TAKEOFF_BIN) list.push(process.env.TAKEOFF_BIN);

	if (process.platform === "darwin") {
		for (const app of APP_NAMES) {
			list.push(`/Applications/${app}.app/Contents/Resources/bin/takeoff`);
			list.push(join(os.homedir(), "Applications", `${app}.app`, "Contents", "Resources", "bin", "takeoff"));
		}
	} else if (process.platform === "win32") {
		const dir = windowsInstallDir();
		if (dir) list.push(join(dir, "takeoff.exe"));
		for (const root of [process.env.ProgramFiles, process.env["ProgramFiles(x86)"], process.env.LOCALAPPDATA]) {
			if (!root) continue;
			for (const app of APP_NAMES) list.push(join(root, app, "takeoff.exe"));
		}
	}

	// Built from source: `cargo install --path crates/takeoff-cli`.
	list.push(join(os.homedir(), ".cargo", "bin", process.platform === "win32" ? "takeoff.exe" : "takeoff"));
	return list;
}

function find() {
	for (const p of candidates()) if (existsSync(p)) return p;
	return onPath(process.platform === "win32" ? "takeoff.exe" : "takeoff");
}

const bin = find();
if (!bin) {
	process.stderr.write(
		[
			"Takeoff Beam is not installed, so its MCP server cannot start.",
			"Install the free app (the `takeoff` CLI ships inside it):",
			"  https://www.construction.live/open",
			"or build the CLI from source:",
			"  cargo install --path crates/takeoff-cli   (https://github.com/constructionlive/takeoffbeam)",
			"If it is installed somewhere unusual, set TAKEOFF_BIN to the binary.",
			"",
		].join("\n"),
	);
	process.exit(1);
}

// Any extra args (a PDF path to open at start) pass straight through.
const child = spawn(bin, ["mcp", ...process.argv.slice(2)], { stdio: "inherit" });
child.on("exit", (code, signal) => process.exit(code ?? (signal ? 1 : 0)));
child.on("error", (err) => {
	process.stderr.write(`Could not start ${bin}: ${err.message}\n`);
	process.exit(1);
});

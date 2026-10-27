//! Sign in to construction.live from Takeoff Desk and use its cloud models.
//!
//! Sign-in is a device code (like a TV app): the app asks the platform for a
//! short code, the person approves it in their browser while signed in to
//! construction.live, and the app polls until it receives a `cld_…` bearer
//! token. The token goes in the OS keychain. Cloud models are then an
//! OpenAI-compatible endpoint at [`chat_base_url`], billed to the account.
//!
//! The platform URL is the Convex HTTP site of the deployment; it defaults to
//! the construction.live desktop default and can be changed with the
//! `TAKEOFF_CLOUD_URL` environment variable or in settings.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_SITE_URL: &str = "https://different-cat-591.convex.site";

#[derive(Clone, Debug, PartialEq)]
pub struct CloudConfig {
	pub site_url: String,
}

impl Default for CloudConfig {
	fn default() -> Self {
		let url = std::env::var("TAKEOFF_CLOUD_URL").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| DEFAULT_SITE_URL.into());
		Self::new(&url)
	}
}

impl CloudConfig {
	pub fn new(site_url: &str) -> Self {
		Self { site_url: site_url.trim().trim_end_matches('/').to_string() }
	}
	fn url(&self, path: &str) -> String {
		format!("{}{}", self.site_url, path)
	}
}

/// OpenAI-compatible base for cloud models: POST `{base}/chat/completions`
/// with `Authorization: Bearer <token>`.
pub fn chat_base_url(cfg: &CloudConfig) -> String {
	cfg.url("/desktop/v1")
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceCode {
	pub device_code: String,
	/// What the person sees/types, e.g. "ABCD-1234".
	pub user_code: String,
	/// Open this in the browser (already carries the code).
	pub verify_url: String,
	pub expires_in: u64,
	/// Seconds between polls.
	pub interval: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Account {
	pub email: String,
	pub name: Option<String>,
	pub team_id: Option<String>,
	pub team_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PollResult {
	Pending,
	Denied,
	Expired,
	Approved { token: String, account: Account },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Me {
	pub email: String,
	pub name: Option<String>,
	pub team: Option<String>,
	pub balance_cents: Option<i64>,
	pub subscription_status: Option<String>,
	pub default_model: Option<String>,
	pub allowed_models: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CloudModel {
	pub id: String,
	pub name: String,
	pub context_tokens: u32,
	pub vision: bool,
}

/// Error strings start with the platform's error code ("unauthorized: …",
/// "insufficient_credits: …", "http_404: …", "network: …") so callers can
/// branch on the prefix.
pub fn is_unauthorized(err: &str) -> bool {
	err.starts_with("unauthorized")
}

fn client() -> Result<reqwest::blocking::Client, String> {
	reqwest::blocking::Client::builder()
		.user_agent(concat!("TakeoffDesk/", env!("CARGO_PKG_VERSION")))
		.timeout(Duration::from_secs(30))
		.build()
		.map_err(|e| format!("network: {e}"))
}

fn read(resp: reqwest::blocking::Response) -> Result<Value, String> {
	let status = resp.status();
	let text = resp.text().map_err(|e| format!("network: {e}"))?;
	let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
	if status.is_success() {
		return Ok(body);
	}
	Err(error_from(status.as_u16(), &body, &text))
}

fn error_from(status: u16, body: &Value, text: &str) -> String {
	match (body["error"]["code"].as_str(), body["error"]["message"].as_str()) {
		(Some(code), Some(msg)) => format!("{code}: {msg}"),
		(Some(code), None) => code.to_string(),
		_ if status == 401 => "unauthorized: sign in again".into(),
		_ => format!("http_{status}: {}", text.chars().take(200).collect::<String>()),
	}
}

fn post(cfg: &CloudConfig, path: &str, body: Value) -> Result<Value, String> {
	read(client()?.post(cfg.url(path)).json(&body).send().map_err(|e| format!("network: {e}"))?)
}

fn get(cfg: &CloudConfig, path: &str, token: &str) -> Result<Value, String> {
	read(client()?.get(cfg.url(path)).bearer_auth(token).send().map_err(|e| format!("network: {e}"))?)
}

fn s(v: &Value) -> Option<String> {
	v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

pub fn start_device_link(cfg: &CloudConfig, device_name: &str) -> Result<DeviceCode, String> {
	parse_device_code(&post(cfg, "/desktop/device/start", serde_json::json!({ "deviceName": device_name }))?)
}

fn parse_device_code(v: &Value) -> Result<DeviceCode, String> {
	Ok(DeviceCode {
		device_code: s(&v["deviceCode"]).ok_or("bad_response: no deviceCode")?,
		user_code: s(&v["userCode"]).ok_or("bad_response: no userCode")?,
		verify_url: s(&v["verifyUrl"]).ok_or("bad_response: no verifyUrl")?,
		expires_in: v["expiresIn"].as_u64().unwrap_or(600),
		interval: v["interval"].as_u64().unwrap_or(3).max(1),
	})
}

/// One poll. Call every `code.interval` seconds until it is not `Pending`.
/// The token is handed out exactly once; save it right away.
pub fn poll(cfg: &CloudConfig, code: &DeviceCode) -> Result<PollResult, String> {
	parse_poll(&post(cfg, "/desktop/device/poll", serde_json::json!({ "deviceCode": code.device_code }))?)
}

fn parse_poll(v: &Value) -> Result<PollResult, String> {
	Ok(match v["status"].as_str() {
		Some("pending") => PollResult::Pending,
		Some("denied") => PollResult::Denied,
		Some("expired") => PollResult::Expired,
		Some("approved") => {
			let a = &v["account"];
			PollResult::Approved {
				token: s(&v["token"]).ok_or("bad_response: approved without a token")?,
				account: Account { email: s(&a["email"]).unwrap_or_default(), name: s(&a["name"]), team_id: s(&a["teamId"]), team_name: s(&a["teamName"]) },
			}
		}
		other => return Err(format!("bad_response: unknown status {other:?}")),
	})
}

pub fn me(cfg: &CloudConfig, token: &str) -> Result<Me, String> {
	parse_me(&get(cfg, "/desktop/v1/me", token)?)
}

fn parse_me(v: &Value) -> Result<Me, String> {
	Ok(Me {
		email: s(&v["email"]).unwrap_or_default(),
		name: s(&v["name"]),
		team: s(&v["team"]["name"]),
		balance_cents: v["credits"]["balanceCents"].as_i64(),
		subscription_status: s(&v["credits"]["subscriptionStatus"]),
		default_model: s(&v["defaultModelId"]),
		allowed_models: v["allowedModelIds"].as_array().map(|a| a.iter().filter_map(s).collect()).unwrap_or_default(),
	})
}

pub fn models(cfg: &CloudConfig, token: &str) -> Result<Vec<CloudModel>, String> {
	parse_models(&get(cfg, "/desktop/v1/models", token)?)
}

fn parse_models(v: &Value) -> Result<Vec<CloudModel>, String> {
	let data = v["data"].as_array().ok_or("bad_response: no data")?;
	Ok(data
		.iter()
		.filter_map(|m| {
			let id = s(&m["id"])?;
			Some(CloudModel {
				name: s(&m["name"]).unwrap_or_else(|| id.clone()),
				context_tokens: m["contextWindowTokens"].as_u64().unwrap_or(128_000) as u32,
				vision: m["vision"].as_bool().unwrap_or(false),
				id,
			})
		})
		.collect())
}

/// The saved sign-in: OS keychain (service "takeoff-desk"), or a 0600 file
/// in the config dir when no keychain is available (headless Linux).
pub mod token {
	use super::Account;

	const SERVICE: &str = "takeoff-desk";
	const USER: &str = "construction.live";

	#[derive(serde::Serialize, serde::Deserialize)]
	struct Saved {
		token: String,
		account: Account,
	}

	/// TAKEOFF_NO_KEYCHAIN (set for cargo test/run in .cargo/config.toml)
	/// keeps the sign-in in the file only: every fresh build is a new binary
	/// to macOS, which would ask for the keychain password each time.
	fn keychain() -> bool {
		std::env::var_os("TAKEOFF_NO_KEYCHAIN").is_none()
	}

	fn fallback_path() -> Option<std::path::PathBuf> {
		Some(dirs::config_dir()?.join("Takeoff Desk").join("cloud-token.json"))
	}

	pub fn save(token: &str, account: &Account) -> Result<(), String> {
		let json = serde_json::to_string(&Saved { token: token.into(), account: account.clone() }).map_err(|e| e.to_string())?;
		if keychain() && keyring::Entry::new(SERVICE, USER).and_then(|e| e.set_password(&json)).is_ok() {
			if let Some(p) = fallback_path() {
				let _ = std::fs::remove_file(p);
			}
			return Ok(());
		}
		let path = fallback_path().ok_or("no config directory for the sign-in")?;
		std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
		std::fs::write(&path, json).map_err(|e| e.to_string())?;
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
		}
		Ok(())
	}

	pub fn load() -> Option<(String, Account)> {
		let raw = keychain()
			.then(|| keyring::Entry::new(SERVICE, USER).and_then(|e| e.get_password()).ok())
			.flatten()
			.or_else(|| std::fs::read_to_string(fallback_path()?).ok())?;
		let saved: Saved = serde_json::from_str(&raw).ok()?;
		Some((saved.token, saved.account))
	}

	pub fn clear() {
		if keychain() {
			if let Ok(e) = keyring::Entry::new(SERVICE, USER) {
				let _ = e.delete_credential();
			}
		}
		if let Some(p) = fallback_path() {
			let _ = std::fs::remove_file(p);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn parses_platform_shapes() {
		// Shapes from convex/desktopGateway.ts + desktopLink.ts.
		let dc = parse_device_code(&json!({"deviceCode": "ab12", "userCode": "ABCD-1234", "verifyUrl": "https://x/desktop/link?code=ABCD-1234", "expiresIn": 600, "interval": 3})).unwrap();
		assert_eq!((dc.user_code.as_str(), dc.interval, dc.expires_in), ("ABCD-1234", 3, 600));
		assert_eq!(parse_poll(&json!({"status": "pending"})).unwrap(), PollResult::Pending);
		assert_eq!(parse_poll(&json!({"status": "expired"})).unwrap(), PollResult::Expired);
		let ok = parse_poll(&json!({"status": "approved", "token": "cld_x", "account": {"email": "a@b.c", "name": null, "teamId": "t1", "teamName": "Acme"}})).unwrap();
		assert!(matches!(ok, PollResult::Approved { ref token, ref account } if token == "cld_x" && account.team_name.as_deref() == Some("Acme") && account.name.is_none()));
		let me = parse_me(&json!({"email": "a@b.c", "name": "Al", "team": {"id": "t", "name": "Acme"}, "credits": {"balanceCents": 1234, "subscriptionStatus": "active"}, "allowedModelIds": ["google/gemini-3.7-flash"], "defaultModelId": "google/gemini-3.7-flash"})).unwrap();
		assert_eq!((me.team.as_deref(), me.balance_cents, me.allowed_models.len()), (Some("Acme"), Some(1234), 1));
		let models = parse_models(&json!({"data": [{"id": "google/gemini-3.7-flash", "name": "Gemini 3.7 Flash", "description": null, "contextWindowTokens": 1048576, "vision": true}, {"id": "x", "name": null, "contextWindowTokens": null, "vision": false}]})).unwrap();
		assert_eq!(models.len(), 2);
		assert!(models[0].vision && models[0].context_tokens == 1_048_576);
		assert_eq!(models[1].name, "x");
		assert_eq!(error_from(401, &json!({"error": {"code": "unauthorized", "message": "Token revoked"}}), ""), "unauthorized: Token revoked");
		assert!(is_unauthorized(&error_from(401, &Value::Null, "")));
		assert_eq!(chat_base_url(&CloudConfig::new("https://x.convex.site/")), "https://x.convex.site/desktop/v1");
	}
}

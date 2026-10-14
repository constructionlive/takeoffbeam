//! Live: ask the platform for a device code (nothing is approved).
fn main() {
	let cfg = takeoff_cloud::CloudConfig::default();
	println!("site: {}", cfg.site_url);
	match takeoff_cloud::start_device_link(&cfg, "Takeoff Desk test") {
		Ok(c) => {
			println!("user_code {} verify_url {} expires_in {} interval {}", c.user_code, c.verify_url, c.expires_in, c.interval);
			println!("first poll: {:?}", takeoff_cloud::poll(&cfg, &c));
		}
		Err(e) => println!("error: {e}"),
	}
	println!("me with a bogus token: {:?}", takeoff_cloud::me(&cfg, "cld_invalid").err());
}

# Launch film

A 74-second, 1080p launch video. It's built entirely from code and the real app, with no screen
recording or video editor:

1. **Footage:** `crates/takeoff-app/tests/footage.rs` drives the real UI with simulated input
   (count, area with snapping, select, layers, AI, models, compare) and saves every frame plus
   the cursor positions.
   ```sh
   FOOTAGE_DIR=$PWD/marketing/footage TAKEOFF_NO_UPDATE_CHECK=1 \
     cargo test --release -p takeoff-app --test footage -- --ignored
   ```
2. **Film:** `marketing/launch-video` composes the type, motion graphics and footage (with a
   drawn cursor and click ripples), synthesizes the score (ambient pad, a pulse in the speed
   section, clicks synced to the cursor), and encodes H.264/AAC with ffmpeg.
   ```sh
   cd marketing/launch-video && cargo run --release -- ../.. ../Takeoff-Desk-launch.mp4
   STILL=41 cargo run --release -- ../.. /tmp/frame.png     # one frame for review
   ```

Every number on screen is measured (see docs/ROADMAP.md).

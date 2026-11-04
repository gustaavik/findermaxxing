//! Runs the cloud API doubles on a local port for manual testing of the app:
//!
//! ```sh
//! cargo run -p mock-cloud --bin mock-cloud-server
//! FM_DEV_CLOUD_MOCK=http://127.0.0.1:<port> cargo run -p app
//! ```
//!
//! Each drive starts with a few sample items.

#![allow(clippy::print_stdout, reason = "a developer tool")]

use mock_cloud::MockCloud;

const FOLDER_MIME: &str = "application/vnd.google-apps.folder";

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let cloud = MockCloud::start().await;
    {
        let mut state = cloud.state();
        let trip = state.google.add("root", "Trip", true, Vec::new(), FOLDER_MIME);
        state.google.add(&trip, "beach.jpg", false, b"not really a photo".to_vec(), "image/jpeg");
        state.google.add("root", "notes.txt", false, b"first".to_vec(), "text/plain");
        state.google.add("root", "notes.txt", false, b"second".to_vec(), "text/plain");
        state.dropbox.add("root", "Readme.md", false, b"# Dropbox".to_vec(), "text/markdown");
        state.onedrive.add("root", "Documents", true, Vec::new(), FOLDER_MIME);
    }
    cloud.add_google_doc("root", "Budget", "spreadsheet");
    println!("FM_DEV_CLOUD_MOCK={}", cloud.base());
    std::future::pending::<()>().await;
}

use std::io;
use yt_dlp::Downloader;
use yt_dlp::client::deps::Libraries;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // let mut url = String::new();
    // io::stdin().read_line(&mut url).unwrap();
    // println!("{url}");

    let executable_dir = PathBuf::from("libs");
    let output_dir = PathBuf::from("output");

    let downloader = Downloader::with_new_binaries(
        executable_dir, 
        output_dir
    ).await?.build().await?;

    Ok(())
}
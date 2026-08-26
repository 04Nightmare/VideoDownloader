use std::io;
use yt_dlp::Downloader;
use yt_dlp::client::deps::{Libraries, LibraryInstaller};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut url = String::new();
    println!("Enter URL: ");
    io::stdin().read_line(&mut url)?;

    let executable_dir = PathBuf::from("libs");
    let output_dir = PathBuf::from("output");

    let youtube_dlp = executable_dir.join("yt-dlp");
    let ffmpeg = executable_dir.join("ffmpeg");

    let libraries = Libraries::new(youtube_dlp, ffmpeg);
    let downloader = Downloader::builder(
        libraries, 
        output_dir
    ).build().await?;

    let url = url.trim();
    let video = downloader.fetch_video_infos_fresh(url).await?;
    let video_path = downloader.download_video(&video, "my-video.mp4").await?;

    Ok(())
}
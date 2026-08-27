use std::io::{self, Write};
use yt_dlp::Downloader;
use yt_dlp::client::deps::{Libraries, LibraryInstaller};
use yt_dlp::model::{AudioQuality, VideoQuality};
use std::path::PathBuf;
use std::fs;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut url = String::new();
    println!("Enter URL: ");
    io::stdin().read_line(&mut url)?;
    let url = url.trim();
    
    let output_dir = PathBuf::from("output");
    
    //Install bins: yt-dlp, ffmpeg.
    let libraries_dir = PathBuf::from("libs");
    let installer = LibraryInstaller::new(libraries_dir);
    installer.install_youtube(None).await?;
    installer.install_ffmpeg(None).await?;
    
    
    let youtube_dlp = PathBuf::from("libs/yt-dlp");
    let ffmpeg = PathBuf::from("libs/ffmepg");
    
    let libraries = Libraries::new(youtube_dlp, ffmpeg);
    let downloader = Downloader::builder(
        libraries, 
        output_dir
    ).build().await?;

    //Fetching video informations.
    println!("Fetching Video Informantion");
    let video = downloader.fetch_video_infos_fresh(url).await?;
    println!("Title: {}", video.title);

    //Original audio
    let original_audio = video
        .formats
        .iter()
        .filter(|format| {
            let is_audio_only = format
                .video_resolution
                .resolution
                .as_deref()
                .map(|resolution| resolution == "audio only")
                .unwrap_or(false);

            let is_original = format
                .format_note
                .as_deref()
                .map(|note| note.contains("original"))
                .unwrap_or(false);

            is_audio_only && is_original
        })
        .max_by(|a, b| {
            let a_rate = a.rates_info.audio_rate.unwrap_or_default();
            let b_rate = b.rates_info.audio_rate.unwrap_or_default();

            a_rate.partial_cmp(&b_rate).unwrap_or(std::cmp::Ordering::Equal)
        })
        .ok_or("No original audio track found")?;
    

    Ok(())
}
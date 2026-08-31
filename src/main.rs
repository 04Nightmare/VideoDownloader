use std::cmp::Ordering::Equal;
use std::io::{self, Write};
use yt_dlp::Downloader;
use yt_dlp::client::deps::{Libraries, LibraryInstaller};
use yt_dlp::model::{AudioQuality, VideoQuality};
use std::path::{self, Path, PathBuf};
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
    
    println!("Completed libs download...");

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

            a_rate.partial_cmp(&b_rate).unwrap_or(Equal)
        })
        .ok_or("No original audio track found")?;
    
    println!("Original audio: {} - {}",
        original_audio.format_id,
        original_audio.format_note.as_deref().unwrap_or("unknown")
    );


    //Vest Video format
    let video_format = video
        .formats
        .iter()
        .filter(|format| {
            format.video_resolution.width.is_some() && format.video_resolution.height.is_some()
        })
        .max_by(|a, b| {
            let a_height = a.video_resolution.height.unwrap_or(0);
            let b_height = b.video_resolution.height.unwrap_or(0);

            a_height.cmp(&b_height)
                .then_with(|| {
                    let a_quality = a.quality_info.quality.unwrap_or(ordered_float::OrderedFloat(0.0));
                    let b_quality = b.quality_info.quality.unwrap_or(ordered_float::OrderedFloat(0.0));

                    a_quality.partial_cmp(&b_quality).unwrap_or(Equal)
                })
        })
        .ok_or("No video format found")?;

        println!("Video format: {} - {}",
            video_format.format_id,
            video_format
                .video_resolution
                .resolution
                .as_deref()
                .unwrap_or("unknown")
        );

        //Temporary paths
        let temp_dir = PathBuf::from("/home/samannyo/Downloads");
        let video_temp = temp_dir.join("video_stream.mp4");
        let audio_temp = temp_dir.join("audio_stream");

        //Final output path
        let video_destinaion = temp_dir.join(format!(
            "{}.mp4",
            video.title
        ));

        //Download video stream
        println!("Downloading video...");

        let video_path = downloader
            .download_format(video_format, video_temp.to_str().unwrap()).await?;
        println!("Video stream downloaded: {:?}", video_path);

        //Download audio stream
        println!("Downloading audio...");

        let audio_path = downloader
            .download_format(original_audio, audio_temp.to_str().unwrap()).await?;
        println!("Audio stream downloaded: {:?}", audio_path);


        //Combine video and audio
        println!("Combining video and original audio...");

        let final_path = downloader
            .combine_audio_and_video_to_path(
                audio_path.to_str().unwrap(), 
                video_path.to_str().unwrap(), 
                video_destinaion.to_str().unwrap()
            ).await?;

        println!("Video Path: {:?}", final_path);


        //clean temp files
        let _ = std::fs::remove_file(video_temp);
        let _ = std::fs::remove_file(audio_temp);
    Ok(())
}
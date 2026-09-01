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
    let ffmpeg = PathBuf::from("libs/ffmpeg");
    
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

    //Finding available audio formats
    let audio_formats: Vec<_> = video
        .formats
        .iter()
        .filter(|format| {
            let is_audio_only = format
                .video_resolution
                .resolution
                .as_deref() == Some("audio only");

            //Filter out HLS/M3U8 streams
            let is_not_hls = format
                .download_info
                .manifest_url
                .is_none();
            is_audio_only && is_not_hls
        }).collect();

        //Error if no audio format found
    if audio_formats.is_empty(){
        return Err("No non-HLS audio track found".into());
    }

    //List of explicitely marked "original"
    let exp_original: Vec<_> = audio_formats
        .iter()
        .filter(|format| {
            format
                .format_note
                .as_deref()
                .map(|note| note.to_lowercase().contains("original"))
                .unwrap_or(false)
        }).copied().collect();
        
    //Prefer original
    let candidates = if !exp_original.is_empty(){
        exp_original
    }else{
        audio_formats.clone()
    };

    //select highest quality audio
    let original_audio = candidates
        .into_iter()
        .max_by(|a, b| {
            let a_rate = a.rates_info.audio_rate.unwrap_or_default();
            let b_rate = b.rates_info.audio_rate.unwrap_or_default();
            a_rate.partial_cmp(&b_rate)
                .unwrap_or(Equal)
                .then_with(|| {
                    let a_quality = a.quality_info.quality.unwrap_or_default();
                    let b_quality = b.quality_info.quality.unwrap_or_default();
                    a_quality
                        .partial_cmp(&b_quality)
                        .unwrap_or(Equal)
                })

        }).ok_or("Could not select an audio format")?;
    
    println!("Original audio: {} - {}",
        original_audio.format_id,
        original_audio.format_note.as_deref().unwrap_or("unknown")
    );


    //Video quality menu

    //Collect all available video resolution.
    let mut resolutions: Vec<u32> = video
        .formats
        .iter()
        .filter_map(|format| {
            format.video_resolution.height
        })
        .filter(|height| *height > 0)
        .collect();

    //Remove duplicate resolutions and sort from heighest to lowest.
    resolutions.sort_unstable();
    resolutions.dedup();
    resolutions.reverse();
    if resolutions.is_empty(){
        return Err("No video formats found".into());
    }

    //Choose quality
    println!();
    println!("Choose video quality: ");
    for (index, resolution) in resolutions.iter().enumerate(){
        println!("{}. {}p", index+1, resolution);
    }

    println!();
    print!("Enter choice: ");
    io::stdout().flush()?;

    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    let choice: usize = choice.trim().parse().map_err(|_| "Invalid choice")?;
    println!();

    if choice == 0 || choice > resolutions.len() {
        return Err("Invalid choice".into());
    }

    let selected_height = resolutions[choice-1];

    //Find the best video format at selected resolution.
    let video_format = video
        .formats
        .iter()
        .filter(|format| {
            format.video_resolution.height == Some(selected_height)
            && format.video_resolution.width.is_some()
        })
        .max_by(|a, b| {
            let a_quality = a.quality_info.quality.unwrap_or_default();
            let b_quality = b.quality_info.quality.unwrap_or_default();
            a_quality
                .partial_cmp(&b_quality)
                .unwrap_or(Equal)
        }).ok_or("Could not find selected video format")?;

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
        let video_destinaion = temp_dir.join("video_download.mp4");

        // let video_destinaion = temp_dir.join(format!(
        //     "{}.mp4",
        //     video.title
        // ));

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
                audio_path, 
                video_path, 
                &video_destinaion,
            ).await?;

        println!("Video Path: {:?}", final_path);


        //clean temp files and dirs
        let _ = std::fs::remove_file(video_temp);
        let _ = std::fs::remove_file(audio_temp);
        let _ = std::fs::remove_dir_all("libs");
        let _ = std::fs::remove_dir("output");

    Ok(())
}
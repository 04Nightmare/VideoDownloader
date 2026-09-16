use std::cmp::Ordering::Equal;
use std::error::Error;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use yt_dlp::model::format::Format;
use yt_dlp::model::Video;
use yt_dlp::Downloader;

type BoxError = Box<dyn Error>;

//Replace character that isn't alphanumeric, space, dash or underscore
fn sanitize_filename(title: &str) -> String {
    title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

//Prompt to type a line and return the trimmed result.
fn prompt_line(prompt: &str) -> io::Result<String> {
    print!("{}", prompt);
    io::stdout().flush()?;
 
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

//Prompt for a menu choice
fn prompt_choice(count: usize) -> Option<usize> {
    println!();
    print!("Enter choice: ");
    io::stdout().flush().ok()?;

    let mut choice = String::new();
    io::stdin().read_line(&mut choice).ok()?;
    let choice: usize = choice.trim().parse().ok()?;
    println!();

    if choice == 0 || choice > count {
        return None;
    }
    Some(choice)
}

//The parsed contents of a line typed at the main prompt
struct UserCommand {
    url: String,
    audio_only: bool,
}

//Parse a raw input line into a normalized YouTube URL plus any flag
fn parse_user_command(input_line: &str) -> Result<UserCommand, BoxError> {
    let mut tokens = input_line.split_whitespace();
    let url_input = tokens.next().unwrap_or_default();
    let audio_only = tokens.any(|arg| arg == "--audio-only");
 
    let url = normalize_youtube_url(url_input)?;
    Ok(UserCommand { url, audio_only })
}

//Convert a `watch?v=` style YouTube URL into the shorter `youtu.be` form.
fn normalize_youtube_url(url_input: &str) -> Result<String, BoxError> {
    if let Some(video_id) = url_input
        .strip_prefix("https://www.youtube.com/watch?v=")
        .map(|rest| rest.split('&').next().unwrap_or(rest))
    {
        Ok(format!("https://youtu.be/{}", video_id))
    } else {
        Ok(url_input.to_string())
    }
}

//Return every non-HLS, audio-only format available for a video.
fn find_audio_formats(video: &Video) -> Vec<&Format> {
    video
        .formats
        .iter()
        .filter(|format| {
            let is_audio_only = format.video_resolution.resolution.as_deref() == Some("audio only");
            let is_not_hls = format.download_info.manifest_url.is_none();
            is_audio_only && is_not_hls
        })
        .collect()
}

//From a list of audio formats, pick the best one
//prefer formats explicitly marked "original", then pick the highest bitrate/quality
fn select_best_audio_format<'a>(audio_formats: &[&'a Format]) -> Option<&'a Format> {
    let exp_original: Vec<&Format> = audio_formats
        .iter()
        .filter(|format| {
            format
                .format_note
                .as_deref()
                .map(|note| note.to_lowercase().contains("original"))
                .unwrap_or(false)
        })
        .copied()
        .collect();
 
    let candidates = if !exp_original.is_empty() {
        exp_original
    } else {
        audio_formats.to_vec()
    };
 
    candidates.into_iter().max_by(|a, b| {
        let a_rate = a.rates_info.audio_rate.unwrap_or_default();
        let b_rate = b.rates_info.audio_rate.unwrap_or_default();
        a_rate
            .partial_cmp(&b_rate)
            .unwrap_or(Equal)
            .then_with(|| {
                let a_quality = a.quality_info.quality.unwrap_or_default();
                let b_quality = b.quality_info.quality.unwrap_or_default();
                a_quality.partial_cmp(&b_quality).unwrap_or(Equal)
            })
    })
}

//Collect distinct video heights (resolutions), sorted from highest to lowest.
fn available_resolutions(video: &Video) -> Vec<u32> {
    let mut resolutions: Vec<u32> = video
        .formats
        .iter()
        .filter_map(|format| format.video_resolution.height)
        .filter(|height| *height > 0)
        .collect();
 
    resolutions.sort_unstable();
    resolutions.dedup();
    resolutions.reverse();
    resolutions
}

//Pick the highest quality video
fn select_best_video_format(video: &Video, height: u32) -> Option<&Format> {
    video
        .formats
        .iter()
        .filter(|format| {
            format.video_resolution.height == Some(height) && format.video_resolution.width.is_some()
        })
        .max_by(|a, b| {
            let a_quality = a.quality_info.quality.unwrap_or_default();
            let b_quality = b.quality_info.quality.unwrap_or_default();
            a_quality.partial_cmp(&b_quality).unwrap_or(Equal)
        })
}

//Choose video quality menu
fn choose_video_format<'a>(video: &'a Video) -> Result<&'a Format, BoxError> {
    let resolutions = available_resolutions(video);
    if resolutions.is_empty() {
        return Err("No video formats found".into());
    }
 
    println!();
    println!("Choose video quality: ");
    for (index, resolution) in resolutions.iter().enumerate() {
        println!("{}. {}p", index + 1, resolution);
    }
 
    let choice = prompt_choice(resolutions.len()).ok_or("Invalid choice")?;
    let selected_height = resolutions[choice - 1];
 
    select_best_video_format(video, selected_height).ok_or_else(|| "Could not find selected video format".into())
}

//Download audio only
async fn download_audio_only(
    downloader: &Downloader,
    audio_format: &Format,
    downloads_dir: &Path,
    sanitized_title: &str,
) -> Result<PathBuf, BoxError> {
    let audio_destination = downloads_dir.join(format!("{}.m4a", sanitized_title));
 
    println!();
    println!("Downloading audio only...");
    let audio_path = downloader
        .download_format(audio_format, audio_destination.to_str().unwrap())
        .await?;
 
    Ok(audio_path)
}

//Download video and audio and merging
async fn download_video_with_audio(
    downloader: &Downloader,
    video_format: &Format,
    audio_format: &Format,
    downloads_dir: &Path,
    sanitized_title: &str,
) -> Result<PathBuf, BoxError> {
    println!(
        "Video format: {} - {}",
        video_format.format_id,
        video_format.video_resolution.resolution.as_deref().unwrap_or("unknown")
    );
 
    let temp_dir = std::env::temp_dir();
    let video_temp = temp_dir.join("video_stream.mp4");
    let audio_temp = temp_dir.join("audio_stream.m4a");
    let video_destination = downloads_dir.join(format!("{}.mp4", sanitized_title));
 
    println!("Downloading video...");
    let video_path = downloader
        .download_format(video_format, video_temp.to_str().unwrap())
        .await?;
    println!("Video stream downloaded: {:?}", video_path);
 
    println!("Downloading audio...");
    let audio_path = downloader
        .download_format(audio_format, audio_temp.to_str().unwrap())
        .await?;
    println!("Audio stream downloaded: {:?}", audio_path);
 
    println!("Combining video and original audio...");
    let final_path = downloader
        .combine_audio_and_video_to_path(audio_path, video_path, &video_destination)
        .await?;
 
    // Clean up temporary files only (preserve libs and output directories)
    let _ = std::fs::remove_file(&video_temp);
    let _ = std::fs::remove_file(&audio_temp);
    let _ = std::fs::remove_dir_all("output");
 
    Ok(final_path)
}

//Handle url, fetch info, pick formats and download
async fn process_url(
    downloader: &Downloader,
    downloads_dir: &Path,
    command: UserCommand,
) -> Result<(), BoxError> {
    println!("Fetching Video Information");
    let video = downloader.fetch_video_infos_fresh(command.url).await?;
    println!("VIDEO TITLE: {}", video.title);
 
    let audio_formats = find_audio_formats(&video);
    if audio_formats.is_empty() {
        return Err("No non-HLS audio track found".into());
    }
 
    let original_audio =
        select_best_audio_format(&audio_formats).ok_or("Could not select an audio format")?;
 
    println!(
        "Original audio: {} - {}",
        original_audio.format_id,
        original_audio.format_note.as_deref().unwrap_or("unknown")
    );
 
    let sanitized_title = sanitize_filename(&video.title);
 
    if command.audio_only {
        let audio_path =
            download_audio_only(downloader, original_audio, downloads_dir, &sanitized_title).await?;
        println!("Audio saved to: {:?}", audio_path);
        println!();
        return Ok(());
    }
 
    let video_format = choose_video_format(&video)?;
    let final_path = download_video_with_audio(
        downloader,
        video_format,
        original_audio,
        downloads_dir,
        &sanitized_title,
    )
    .await?;
 
    println!("Video saved to: {:?}", final_path);
    println!();
 
    Ok(())
}

//Downlaods the binaries
async fn setup_bins_downloader() -> Result<Downloader, BoxError> {
    let output_dir = PathBuf::from("output");
    let libraries_dir = PathBuf::from("libs");
 
    println!("Installing binaries...");
    let downloader = Downloader::with_new_binaries(libraries_dir, output_dir)
        .await?
        .build()
        .await?;
    println!("Libraries ready.");
    println!();
 
    Ok(downloader)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
   let downloader = setup_bins_downloader().await?;
    let downloads_dir = dirs::download_dir().ok_or("Could not determine the downloads directory")?;
 
    loop {
        let input_line = prompt_line("Enter URL [--audio-only] (or 'exit', 'quit'): ")?;
 
        if input_line.is_empty()
            || input_line.eq_ignore_ascii_case("exit")
            || input_line.eq_ignore_ascii_case("quit")
        {
            break;
        }
 
        let command = match parse_user_command(&input_line) {
            Ok(command) => command,
            Err(err) => {
                eprintln!("{}", err);
                continue;
            }
        };
 
        if let Err(err) = process_url(&downloader, &downloads_dir, command).await {
            eprintln!("{}", err);
            continue;
        }
    }
 
    println!("Goodbye!! UwU...");
    thread::sleep(Duration::from_secs(2));
    Ok(())
}